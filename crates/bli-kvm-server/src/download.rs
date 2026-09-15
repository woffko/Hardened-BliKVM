//! Image-download capability flags and the operator image upload.
//!
//! The flags keep the Hardened NanoKVM `ImageEnabledRsp` shape. Remote image
//! download (`POST /api/download/image`) is not a shipped capability and stays
//! disabled. The local upload (`POST /api/download/file`) accepts one bounded
//! multipart file, writes it atomically into the configured image directory,
//! hashes it while streaming and records it in the media catalog.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use axum::{
    Json,
    extract::{Multipart, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use bli_kvm_media_catalog::{CatalogImage, MediaCatalog};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::error::{ApiResponse, AppError, AppResult};

/// Hard cap for one uploaded image.
pub(crate) const MAX_MEDIA_UPLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Slack allowed over `Content-Length` for multipart framing.
const CONTENT_LENGTH_SLACK: u64 = 1024 * 1024;
const MAX_UPLOAD_NAME_BYTES: usize = 64;

static UPLOAD_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UploadKind {
    Iso,
    MassStorage,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageUploadResponse {
    pub image_id: u64,
    pub image_name: String,
    pub size: u64,
}

/// Accept only `[A-Za-z0-9][A-Za-z0-9._-]*` with an `img`/`iso` extension.
pub(crate) fn validate_upload_name(value: &str) -> Result<UploadKind, AppError> {
    let bytes = value.as_bytes();
    if !(1..=MAX_UPLOAD_NAME_BYTES).contains(&bytes.len())
        || !bytes[0].is_ascii_alphanumeric()
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-'))
        || value.contains("..")
    {
        return Err(AppError::BadRequest("image name is invalid".into()));
    }
    match value.rsplit_once('.') {
        Some((_, extension)) if extension.eq_ignore_ascii_case("iso") => Ok(UploadKind::Iso),
        Some((_, extension)) if extension.eq_ignore_ascii_case("img") => {
            Ok(UploadKind::MassStorage)
        }
        _ => Err(AppError::BadRequest(
            "only .img and .iso images are accepted".into(),
        )),
    }
}

pub(crate) fn upload_allows(kind: UploadKind) -> (bool, bool, bool) {
    match kind {
        UploadKind::Iso => (true, true, false),
        UploadKind::MassStorage => (false, true, false),
    }
}

fn temporary_path(directory: &Path, name: &str) -> PathBuf {
    let sequence = UPLOAD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    directory.join(format!(".{name}.{sequence}.upload"))
}

pub(crate) async fn upload_image(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    match upload_image_inner(&state, &headers, &mut multipart).await {
        Ok(response) => response.into_response(),
        Err(error) => error.into_response(),
    }
}

async fn upload_image_inner(
    state: &AppState,
    headers: &HeaderMap,
    multipart: &mut Multipart,
) -> AppResult<Json<ApiResponse<ImageUploadResponse>>> {
    if !state.options.restricted_controls_enabled
        || !state.options.media_controls_enabled
        || state.control.is_none()
    {
        return Err(AppError::ServiceUnavailable(
            "image upload is not available".into(),
        ));
    }
    let directory =
        state.options.media_directory.clone().ok_or_else(|| {
            AppError::ServiceUnavailable("image storage is not configured".into())
        })?;
    let catalog_path =
        state.options.media_catalog.clone().ok_or_else(|| {
            AppError::ServiceUnavailable("image catalog is not configured".into())
        })?;
    if let Some(length) = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        && length > MAX_MEDIA_UPLOAD_BYTES + CONTENT_LENGTH_SLACK
    {
        return Err(AppError::BadRequest("upload is too large".into()));
    }

    let mut uploaded: Option<(String, UploadKind, u64, String)> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| AppError::BadRequest("invalid multipart data".into()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        if uploaded.is_some() {
            return Err(AppError::BadRequest("only one file is allowed".into()));
        }
        let name = field.file_name().unwrap_or_default().to_owned();
        let kind = validate_upload_name(&name)?;
        let temporary = temporary_path(&directory, &name);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| AppError::ServiceUnavailable("image storage is unavailable".into()))?;
        let mut hasher = Sha256::new();
        let mut total: u64 = 0;
        let mut field = field;
        let outcome = async {
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|_| AppError::BadRequest("invalid multipart chunk".into()))?
            {
                total = total.saturating_add(chunk.len() as u64);
                if total > MAX_MEDIA_UPLOAD_BYTES {
                    return Err(AppError::BadRequest("upload is too large".into()));
                }
                hasher.update(&chunk);
                file.write_all(&chunk).map_err(|_| {
                    AppError::ServiceUnavailable("image storage is unavailable".into())
                })?;
            }
            Ok(())
        }
        .await;
        match outcome {
            Ok(()) => {
                let digest = hasher.finalize();
                let mut sha256 = String::with_capacity(64);
                for byte in digest {
                    sha256.push_str(&format!("{byte:02x}"));
                }
                let target = directory.join(&name);
                file.sync_all().map_err(|_| {
                    AppError::ServiceUnavailable("image storage is unavailable".into())
                })?;
                drop(file);
                std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o644))
                    .map_err(|_| {
                        AppError::ServiceUnavailable("image storage is unavailable".into())
                    })?;
                std::fs::rename(&temporary, &target).map_err(|_| {
                    AppError::ServiceUnavailable("image storage is unavailable".into())
                })?;
                uploaded = Some((name, kind, total, sha256));
            }
            Err(error) => {
                drop(file);
                let _ = std::fs::remove_file(&temporary);
                return Err(error);
            }
        }
    }

    let Some((name, kind, size, sha256)) = uploaded else {
        return Err(AppError::BadRequest("no file part found".into()));
    };
    let (cdrom_allowed, removable_allowed, writable_allowed) = upload_allows(kind);
    let summary = MediaCatalog::upsert_image(
        &catalog_path,
        &CatalogImage {
            image_name: name,
            size,
            sha256,
            cdrom_allowed,
            removable_allowed,
            writable_allowed,
        },
    )
    .map_err(|_| AppError::ServiceUnavailable("image catalog is unavailable".into()))?;
    Ok(Json(ApiResponse::ok(ImageUploadResponse {
        image_id: summary.image_id,
        image_name: summary.image_name,
        size: summary.size,
    })))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageEnabledResponse {
    enabled: bool,
    remote_enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct RemoteEnabledResponse {
    enabled: bool,
}

pub async fn image_enabled(
    State(state): State<AppState>,
) -> Json<ApiResponse<ImageEnabledResponse>> {
    Json(ApiResponse::ok(ImageEnabledResponse {
        enabled: state.options.media_catalog.is_some(),
        remote_enabled: false,
    }))
}

pub async fn remote_enabled() -> Json<ApiResponse<RemoteEnabledResponse>> {
    Json(ApiResponse::ok(RemoteEnabledResponse { enabled: false }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_download_stays_disabled() {
        let response = RemoteEnabledResponse { enabled: false };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["enabled"], false);
    }

    #[test]
    fn image_enabled_response_is_camel_case() {
        let json = serde_json::to_value(ImageEnabledResponse {
            enabled: true,
            remote_enabled: false,
        })
        .unwrap();
        assert_eq!(json["enabled"], true);
        assert_eq!(json["remoteEnabled"], false);
    }

    #[test]
    fn upload_names_are_validated_and_typed() {
        assert_eq!(
            validate_upload_name("kubuntu.img").unwrap(),
            UploadKind::MassStorage
        );
        assert_eq!(
            validate_upload_name("debian-12.ISO").unwrap(),
            UploadKind::Iso
        );
        assert_eq!(
            validate_upload_name("a_b-c.Img").unwrap(),
            UploadKind::MassStorage
        );
        for invalid in [
            "",
            "../escape.img",
            ".hidden.img",
            "name with space.img",
            "image.raw",
            "image.img.gz",
        ] {
            with_rejected(invalid);
        }
        with_rejected(&"a".repeat(MAX_UPLOAD_NAME_BYTES + 1));
    }

    fn with_rejected(name: &str) {
        assert!(
            validate_upload_name(name).is_err(),
            "expected {name:?} to be rejected"
        );
    }

    #[test]
    fn upload_modes_follow_the_extension() {
        assert_eq!(upload_allows(UploadKind::Iso), (true, true, false));
        assert_eq!(upload_allows(UploadKind::MassStorage), (false, true, false));
    }

    #[test]
    fn temporary_paths_are_unique_and_hidden() {
        let directory = Path::new("/tmp/images");
        let first = temporary_path(directory, "kubuntu.img");
        let second = temporary_path(directory, "kubuntu.img");
        assert_ne!(first, second);
        for path in [first, second] {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.starts_with('.'));
            assert!(name.ends_with(".upload"));
            assert_eq!(path.parent(), Some(directory));
        }
    }
}
