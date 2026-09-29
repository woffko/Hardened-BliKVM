//! Image-download capability flags and the operator image upload.
//!
//! The flags keep the Hardened NanoKVM `ImageEnabledRsp` shape. Remote image
//! download (`POST /api/download/image`, `remote_download.rs`) is an
//! administration feature, off until switched on. The local upload (`POST /api/download/file`) accepts one bounded
//! multipart file, writes it atomically into the configured image directory,
//! hashes it while streaming and records it in the media catalog.
//! `POST /api/download/file/check` answers the same refusals for a name and a
//! size before the web client sends a file: an answer the server gives while
//! a large body is still arriving does not reliably reach the browser.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

use axum::{
    Extension, Json,
    extract::{Multipart, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use bli_kvm_media_catalog::{CatalogImage, MediaCatalog};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::auth::middleware::CurrentSession;
use crate::error::{ApiResponse, AppError, AppResult};

/// Hard cap for one uploaded image; the image store's free space is the
/// usual limit.
pub(crate) const MAX_MEDIA_UPLOAD_BYTES: u64 = 64 * 1024 * 1024 * 1024;
/// Slack allowed over `Content-Length` for multipart framing.
const CONTENT_LENGTH_SLACK: u64 = 1024 * 1024;
/// The media catalog's limit for an image name.
const MAX_UPLOAD_NAME_BYTES: usize = 128;
/// The web client maps this reason to the operator's language.
const NO_SPACE: &str = "the image store does not have enough free space";

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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UploadCheck {
    name: String,
    size: u64,
}

/// The refusals an upload of `name` with `size` bytes would meet.
pub(crate) async fn check_upload(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(check): Json<UploadCheck>,
) -> AppResult<Json<ApiResponse<()>>> {
    let directory = upload_directory(&state)?;
    validate_upload_name(&check.name)?;
    if check.size == 0 {
        return Err(AppError::BadRequest("the file is empty".into()));
    }
    if check.size > MAX_MEDIA_UPLOAD_BYTES {
        return Err(AppError::BadRequest("upload is too large".into()));
    }
    if let Some(free) = store_free_bytes(&directory)
        && check.size > free
    {
        return Err(AppError::Conflict(NO_SPACE.into()));
    }
    refuse_attached(&state, &session.token, &check.name).await?;
    Ok(Json(ApiResponse::ok(())))
}

/// The image directory, when uploads are available at all.
fn upload_directory(state: &AppState) -> AppResult<PathBuf> {
    if !state.options.restricted_controls_enabled
        || !state.options.media_controls_enabled
        || state.control.is_none()
    {
        return Err(AppError::ServiceUnavailable(
            "image upload is not available".into(),
        ));
    }
    state
        .options
        .media_directory
        .clone()
        .ok_or_else(|| AppError::ServiceUnavailable("image storage is not configured".into()))
}

/// Replacing the attached image would leave the target reading the old file
/// while the catalog describes the new one.
async fn refuse_attached(state: &AppState, session_token: &str, name: &str) -> AppResult<()> {
    if attached_image(state, session_token).await?.as_deref() == Some(name) {
        return Err(AppError::Conflict(
            "detach the drive before replacing its image".into(),
        ));
    }
    Ok(())
}

pub(crate) fn upload_allows(kind: UploadKind) -> (bool, bool, bool) {
    match kind {
        UploadKind::Iso => (true, true, false),
        UploadKind::MassStorage => (false, true, false),
    }
}

/// A name of its own per upload. A counter restarting with the process would
/// meet the partial file an interrupted upload left and refuse the same image
/// again.
fn temporary_path(directory: &Path, name: &str) -> PathBuf {
    let unique = crate::auth::token::random_token(9);
    directory.join(format!(".{name}.{unique}.upload"))
}

/// Partial uploads older than this are leftovers of an interrupted process.
const STALE_UPLOAD_AGE: std::time::Duration = std::time::Duration::from_secs(3600);

/// Removes the partial files interrupted uploads left in the image store.
pub(crate) fn remove_stale_uploads(directory: &Path, age: std::time::Duration) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with('.') && name.ends_with(".upload")) {
            continue;
        }
        let old = entry
            .metadata()
            .ok()
            .filter(|metadata| metadata.is_file())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|elapsed| elapsed >= age);
        if old && std::fs::remove_file(entry.path()).is_ok() {
            eprintln!("media: removed the partial upload {name}");
        }
    }
}

/// The startup sweep of the image store.
pub(crate) fn sweep_stale_uploads(directory: &Path) {
    remove_stale_uploads(directory, STALE_UPLOAD_AGE);
}

pub(crate) async fn upload_image(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    match upload_image_inner(&state, &session.token, &headers, &mut multipart).await {
        Ok(response) => response.into_response(),
        Err(error) => {
            // nginx passes the body on unbuffered and reads the answer only
            // once the body is sent: a refusal with the body left unread
            // stalls the upload instead of reaching the browser.
            drain(&mut multipart).await;
            error.into_response()
        }
    }
}

/// Reads and discards what is left of an upload.
async fn drain(multipart: &mut Multipart) {
    while let Ok(Some(mut field)) = multipart.next_field().await {
        while let Ok(Some(_)) = field.chunk().await {}
    }
}

#[cfg(feature = "restricted-control-surface")]
async fn attached_image(state: &AppState, session_token: &str) -> AppResult<Option<String>> {
    crate::media::attached_image(state, session_token).await
}

/// Without the restricted control surface nothing can be attached.
#[cfg(not(feature = "restricted-control-surface"))]
async fn attached_image(_state: &AppState, _session_token: &str) -> AppResult<Option<String>> {
    Ok(None)
}

/// The image store cannot take a file: the operator is told so, the board log
/// keeps the cause.
fn store_unwritable(name: &str, error: &std::io::Error) -> AppError {
    eprintln!("media: {name} could not be stored: {error}");
    if error.raw_os_error() == Some(libc::ENOSPC) {
        return AppError::Conflict(NO_SPACE.into());
    }
    AppError::Conflict("the image store on the board is not writable".into())
}

/// Bytes the server's user may still write into the image store.
pub(crate) fn store_free_bytes(directory: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(directory.as_os_str().as_bytes()).ok()?;
    // SAFETY: `statvfs` only writes into the zeroed structure.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    Some((stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64))
}

/// Whether a request body of `length` bytes can hold a file that fits into
/// `free` bytes; the multipart framing stays under the slack.
fn fits_store(length: u64, free: u64) -> bool {
    length <= free.saturating_add(CONTENT_LENGTH_SLACK)
}

async fn upload_image_inner(
    state: &AppState,
    session_token: &str,
    headers: &HeaderMap,
    multipart: &mut Multipart,
) -> AppResult<Json<ApiResponse<ImageUploadResponse>>> {
    let directory = upload_directory(state)?;
    let catalog_path =
        state.options.media_catalog.clone().ok_or_else(|| {
            AppError::ServiceUnavailable("image catalog is not configured".into())
        })?;
    let length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if let Some(length) = length
        && length > MAX_MEDIA_UPLOAD_BYTES + CONTENT_LENGTH_SLACK
    {
        return Err(AppError::BadRequest("upload is too large".into()));
    }
    // Refused before the body is read; the web client checks the same free
    // space first, since a proxy may not pass an early answer on.
    if let Some(length) = length
        && let Some(free) = store_free_bytes(&directory)
        && !fits_store(length, free)
    {
        return Err(AppError::Conflict(NO_SPACE.into()));
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
        refuse_attached(state, session_token, &name).await?;
        let temporary = temporary_path(&directory, &name);
        // Writes and the final sync go through the blocking pool: on the card
        // they can stall for seconds and must not hold an async worker.
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await
            .map_err(|error| store_unwritable(&name, &error))?;
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
                file.write_all(&chunk)
                    .await
                    .map_err(|error| store_unwritable(&name, &error))?;
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
                if let Err(error) = file.sync_all().await {
                    drop(file);
                    let _ = tokio::fs::remove_file(&temporary).await;
                    return Err(store_unwritable(&name, &error));
                }
                drop(file);
                // The drive may have been attached to this image while it was
                // arriving; the rename would swap the file under the target.
                if let Err(error) = refuse_attached(state, session_token, &name).await {
                    let _ = tokio::fs::remove_file(&temporary).await;
                    return Err(error);
                }
                if let Err(error) =
                    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o644))
                        .and_then(|()| std::fs::rename(&temporary, &target))
                {
                    let _ = std::fs::remove_file(&temporary);
                    return Err(store_unwritable(&name, &error));
                }
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
    /// Free space in the image store, when it can be read.
    free_bytes: Option<u64>,
    max_upload_bytes: u64,
}

pub async fn image_enabled(
    State(state): State<AppState>,
) -> Json<ApiResponse<ImageEnabledResponse>> {
    Json(ApiResponse::ok(ImageEnabledResponse {
        enabled: state.options.media_catalog.is_some(),
        // Remote download is an administration feature, off until switched on.
        remote_enabled: crate::admin_features::switches().remote_download,
        free_bytes: state
            .options
            .media_directory
            .as_deref()
            .and_then(store_free_bytes),
        max_upload_bytes: MAX_MEDIA_UPLOAD_BYTES,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_enabled_response_is_camel_case() {
        let json = serde_json::to_value(ImageEnabledResponse {
            enabled: true,
            remote_enabled: false,
            free_bytes: Some(7),
            max_upload_bytes: MAX_MEDIA_UPLOAD_BYTES,
        })
        .unwrap();
        assert_eq!(json["enabled"], true);
        assert_eq!(json["remoteEnabled"], false);
        assert_eq!(json["freeBytes"], 7);
        assert_eq!(json["maxUploadBytes"], MAX_MEDIA_UPLOAD_BYTES);
    }

    #[test]
    fn an_upload_is_refused_when_the_store_cannot_hold_it() {
        let free = 2 * 1024 * 1024 * 1024;
        assert!(fits_store(free, free));
        assert!(fits_store(free + CONTENT_LENGTH_SLACK, free));
        assert!(!fits_store(free + CONTENT_LENGTH_SLACK + 1, free));
        assert!(store_free_bytes(Path::new("/")).is_some());
        assert!(store_free_bytes(Path::new("/nonexistent/image/store")).is_none());
    }

    #[test]
    fn a_full_store_is_named() {
        let full = std::io::Error::from_raw_os_error(libc::ENOSPC);
        assert!(
            matches!(store_unwritable("a.iso", &full), AppError::Conflict(message) if message == NO_SPACE)
        );
        let denied = std::io::Error::from_raw_os_error(libc::EACCES);
        assert!(matches!(
            store_unwritable("a.iso", &denied),
            AppError::Conflict(message) if message == "the image store on the board is not writable"
        ));
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
        // Distribution images carry long names.
        validate_upload_name(
            "FreeBSD-15.0-PRERELEASE-amd64-20250822-027be99b1f33-279771-disc1.iso",
        )
        .unwrap();
        validate_upload_name(&format!("{}.iso", "a".repeat(MAX_UPLOAD_NAME_BYTES - 4))).unwrap();
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

#[cfg(test)]
mod leftover_tests {
    use super::*;

    #[test]
    fn temporary_names_are_unique_and_stale_partials_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let first = temporary_path(dir.path(), "a.iso");
        let second = temporary_path(dir.path(), "a.iso");
        assert_ne!(first, second);
        assert!(first.to_string_lossy().ends_with(".upload"));

        let old = dir.path().join(".a.iso.0.upload");
        let fresh = dir.path().join(".b.iso.1.upload");
        let image = dir.path().join("keep.iso");
        for path in [&old, &fresh, &image] {
            std::fs::write(path, b"x").unwrap();
        }
        let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&image)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        remove_stale_uploads(dir.path(), STALE_UPLOAD_AGE);
        assert!(!old.exists(), "an old partial upload goes");
        assert!(fresh.exists(), "a recent one may still be arriving");
        assert!(image.exists(), "an image is never touched");
    }
}
