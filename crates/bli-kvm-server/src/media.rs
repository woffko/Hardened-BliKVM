//! Virtual-media control endpoint for the restricted control surface.
//!
//! The endpoint forwards a typed media command to the control broker as a v2
//! `VirtualMedia` request. The command carries an opaque image ID, never a path
//! or filename. The server derives the session binding from the authenticated
//! session and a stable domain-separated target binding, because virtual media
//! is not focus-bound like HID.
//!
//! No media command is accepted unless the restricted control surface is
//! compiled in, explicitly enabled and backed by a configured control socket.

use axum::{Extension, Json, extract::State};
use bli_kvm_core::VirtualMediaCommand;
use serde::{Deserialize, Serialize};

#[cfg(feature = "restricted-control-surface")]
use bli_kvm_control_protocol::ControlResponse;
#[cfg(feature = "restricted-control-surface")]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "restricted-control-surface")]
use bli_kvm_core::{MediaRequest, PlatformControlRequestV2};
#[cfg(feature = "restricted-control-surface")]
use sha2::{Digest, Sha256};

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
};

#[cfg(feature = "restricted-control-surface")]
use bli_kvm_core::MediaObserveRequest;
#[cfg(feature = "restricted-control-surface")]
use std::path::PathBuf;

#[cfg(feature = "restricted-control-surface")]
use crate::hid::{rejection_name, session_identity, valid_operation_id};

#[cfg(feature = "restricted-control-surface")]
const MEDIA_TARGET_DOMAIN: &[u8] = b"hardened-blikvm-media-target-v1";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) struct VirtualMediaBody {
    pub operation_id: String,
    pub command: VirtualMediaCommand,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VirtualMediaOutcome {
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<&'static str>,
}

#[cfg(feature = "restricted-control-surface")]
fn media_target_binding() -> [u8; 32] {
    let digest = Sha256::digest(MEDIA_TARGET_DOMAIN);
    let mut binding = [0_u8; 32];
    binding.copy_from_slice(&digest);
    binding
}

#[cfg(feature = "restricted-control-surface")]
fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) async fn virtual_media(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(request): Json<VirtualMediaBody>,
) -> AppResult<Json<ApiResponse<VirtualMediaOutcome>>> {
    #[cfg(not(feature = "restricted-control-surface"))]
    {
        let _ = (&state, &session, &request);
        Err(AppError::ServiceUnavailable(
            "virtual media control is not available".into(),
        ))
    }
    #[cfg(feature = "restricted-control-surface")]
    {
        let Some(control) = state.control.as_ref() else {
            return Err(AppError::ServiceUnavailable(
                "virtual media control is not configured".into(),
            ));
        };
        if !state.options.restricted_controls_enabled || !state.options.media_controls_enabled {
            return Err(AppError::ServiceUnavailable(
                "virtual media control is not enabled".into(),
            ));
        }
        if !valid_operation_id(&request.operation_id) {
            return Err(AppError::BadRequest("media operation ID is invalid".into()));
        }
        let media = MediaRequest {
            command: request.command,
            session_binding: session_identity(&session.token),
            target_binding: media_target_binding(),
            issued_at_unix_ms: unix_millis(),
        };
        let response = control
            .execute_v2(
                &request.operation_id,
                PlatformControlRequestV2::VirtualMedia { request: media },
            )
            .await
            .map_err(|_| {
                AppError::ServiceUnavailable("virtual media control is unavailable".into())
            })?;
        Ok(Json(ApiResponse::ok(match response {
            ControlResponse::Applied | ControlResponse::LeaseGranted { .. } => {
                VirtualMediaOutcome {
                    applied: true,
                    rejection: None,
                }
            }
            ControlResponse::Rejected { code } => VirtualMediaOutcome {
                applied: false,
                rejection: Some(rejection_name(code)),
            },
            ControlResponse::MediaState { .. }
            | ControlResponse::HidReconciliationState { .. }
            | ControlResponse::BoardStatus { .. }
            | ControlResponse::VideoState { .. }
            | ControlResponse::MdnsState { .. }
            | ControlResponse::MouseJigglerState { .. }
            | ControlResponse::HidOnlyState { .. }
            | ControlResponse::WifiState { .. }
            | ControlResponse::TlsState { .. }
            | ControlResponse::FirewallModeState { .. }
            | ControlResponse::FirewallState { .. }
            | ControlResponse::TailscaleState { .. } => VirtualMediaOutcome {
                applied: false,
                rejection: Some("invalid_state"),
            },
        })))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaCatalogView {
    pub images: Vec<bli_kvm_media_catalog::MediaSummary>,
}

/// Read-only opaque-ID virtual-media catalog for the operator UI.
pub(crate) async fn media_catalog(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<MediaCatalogView>>> {
    #[cfg(feature = "restricted-control-surface")]
    {
        if !state.options.restricted_controls_enabled {
            return Err(AppError::ServiceUnavailable(
                "virtual media catalog is not enabled".into(),
            ));
        }
        let Some(path) = state.options.media_catalog.as_ref() else {
            return Err(AppError::ServiceUnavailable(
                "virtual media catalog is not configured".into(),
            ));
        };
        let catalog = bli_kvm_media_catalog::MediaCatalog::open(path).map_err(|_| {
            AppError::ServiceUnavailable("virtual media catalog is unavailable".into())
        })?;
        Ok(Json(ApiResponse::ok(MediaCatalogView {
            images: catalog.summaries(),
        })))
    }
    #[cfg(not(feature = "restricted-control-surface"))]
    {
        let _ = &state;
        Err(AppError::ServiceUnavailable(
            "virtual media catalog is not available".into(),
        ))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) struct StorageMountBody {
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub cdrom: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) struct StorageDeleteBody {
    pub file: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageImagesView {
    pub files: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageMountedView {
    pub file: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageCdromView {
    pub cdrom: u8,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageOutcome {
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<&'static str>,
}

/// Observed media state over the control socket (reference storage surface).
#[cfg(feature = "restricted-control-surface")]
pub(crate) struct MediaObserved {
    pub file: Option<String>,
    pub connected: bool,
    pub cdrom: bool,
}

#[cfg(feature = "restricted-control-surface")]
fn storage_catalog_path(state: &AppState) -> AppResult<&std::path::Path> {
    state.options.media_catalog.as_deref().ok_or_else(|| {
        AppError::ServiceUnavailable("virtual media catalog is not configured".into())
    })
}

#[cfg(feature = "restricted-control-surface")]
fn storage_operation_id(session_token: &str, domain: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(session_token.as_bytes());
    hasher.update(unix_millis().to_le_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(feature = "restricted-control-surface")]
async fn observe_media(state: &AppState, session_token: &str) -> AppResult<MediaObserved> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "virtual media control is not configured".into(),
        ));
    };
    if !state.options.restricted_controls_enabled || !state.options.media_controls_enabled {
        return Err(AppError::ServiceUnavailable(
            "virtual media control is not enabled".into(),
        ));
    }
    let request = MediaObserveRequest {
        session_binding: session_identity(session_token),
        target_binding: media_target_binding(),
        issued_at_unix_ms: unix_millis(),
    };
    let response = control
        .execute_v2(
            storage_operation_id(session_token, b"hardened-blikvm-media-observe-v1"),
            PlatformControlRequestV2::VirtualMediaObserve { request },
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("virtual media control is unavailable".into()))?;
    match response {
        ControlResponse::MediaState {
            file,
            connected,
            cdrom,
            ..
        } => Ok(MediaObserved {
            file,
            connected,
            cdrom,
        }),
        ControlResponse::Rejected { code } => Err(AppError::ServiceUnavailable(format!(
            "virtual media state is unavailable: {}",
            rejection_name(code)
        ))),
        _ => Err(AppError::ServiceUnavailable(
            "virtual media state is unavailable".into(),
        )),
    }
}

#[cfg(feature = "restricted-control-surface")]
async fn apply_media_command(
    state: &AppState,
    session_token: &str,
    command: bli_kvm_core::VirtualMediaCommand,
    domain: &[u8],
) -> AppResult<StorageOutcome> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "virtual media control is not configured".into(),
        ));
    };
    if !state.options.restricted_controls_enabled || !state.options.media_controls_enabled {
        return Err(AppError::ServiceUnavailable(
            "virtual media control is not enabled".into(),
        ));
    }
    let media = MediaRequest {
        command,
        session_binding: session_identity(session_token),
        target_binding: media_target_binding(),
        issued_at_unix_ms: unix_millis(),
    };
    let response = control
        .execute_v2(
            storage_operation_id(session_token, domain),
            PlatformControlRequestV2::VirtualMedia { request: media },
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("virtual media control is unavailable".into()))?;
    Ok(match response {
        ControlResponse::Applied => StorageOutcome {
            applied: true,
            rejection: None,
        },
        ControlResponse::Rejected { code } => StorageOutcome {
            applied: false,
            rejection: Some(rejection_name(code)),
        },
        _ => StorageOutcome {
            applied: false,
            rejection: Some("invalid_state"),
        },
    })
}

/// Image list in the reference shape (`data.files`).
pub(crate) async fn storage_images(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<StorageImagesView>>> {
    #[cfg(feature = "restricted-control-surface")]
    {
        if !state.options.restricted_controls_enabled || !state.options.media_controls_enabled {
            return Err(AppError::ServiceUnavailable(
                "virtual media control is not enabled".into(),
            ));
        }
        let catalog = bli_kvm_media_catalog::MediaCatalog::open(storage_catalog_path(&state)?)
            .map_err(|_| {
                AppError::ServiceUnavailable("virtual media catalog is unavailable".into())
            })?;
        Ok(Json(ApiResponse::ok(StorageImagesView {
            files: catalog
                .summaries()
                .into_iter()
                .map(|entry| entry.image_name)
                .collect(),
        })))
    }
    #[cfg(not(feature = "restricted-control-surface"))]
    {
        let _ = &state;
        Err(AppError::ServiceUnavailable(
            "virtual media control is not available".into(),
        ))
    }
}

/// Currently mounted image in the reference shape (`data.file`).
pub(crate) async fn storage_mounted(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<StorageMountedView>>> {
    #[cfg(feature = "restricted-control-surface")]
    {
        let observed = observe_media(&state, &session.token).await?;
        Ok(Json(ApiResponse::ok(StorageMountedView {
            file: observed.file.unwrap_or_default(),
        })))
    }
    #[cfg(not(feature = "restricted-control-surface"))]
    {
        let _ = (&state, &session);
        Err(AppError::ServiceUnavailable(
            "virtual media control is not available".into(),
        ))
    }
}

/// CD-ROM flag of the mounted image in the reference shape (`data.cdrom`).
pub(crate) async fn storage_cdrom(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<StorageCdromView>>> {
    #[cfg(feature = "restricted-control-surface")]
    {
        let observed = observe_media(&state, &session.token).await?;
        Ok(Json(ApiResponse::ok(StorageCdromView {
            cdrom: u8::from(observed.cdrom && observed.connected),
        })))
    }
    #[cfg(not(feature = "restricted-control-surface"))]
    {
        let _ = (&state, &session);
        Err(AppError::ServiceUnavailable(
            "virtual media control is not available".into(),
        ))
    }
}

/// Mount or unmount an image in one command (empty file unmounts).
pub(crate) async fn storage_mount(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<StorageMountBody>,
) -> AppResult<Json<ApiResponse<StorageOutcome>>> {
    #[cfg(feature = "restricted-control-surface")]
    {
        let command = if body.file.is_empty() {
            bli_kvm_core::VirtualMediaCommand::SetConnected { connected: false }
        } else {
            let catalog = bli_kvm_media_catalog::MediaCatalog::open(storage_catalog_path(&state)?)
                .map_err(|_| {
                    AppError::ServiceUnavailable("virtual media catalog is unavailable".into())
                })?;
            let Some(entry) = catalog.find_by_name(&body.file) else {
                return Err(AppError::BadRequest(
                    "image is not present in the catalog".into(),
                ));
            };
            let mode = if body.cdrom {
                bli_kvm_core::VirtualMediaControlMode::CdRom
            } else {
                bli_kvm_core::VirtualMediaControlMode::RemovableDisk
            };
            let allowed = match mode {
                bli_kvm_core::VirtualMediaControlMode::CdRom => entry.cdrom_allowed,
                bli_kvm_core::VirtualMediaControlMode::RemovableDisk => entry.removable_allowed,
            };
            if !allowed {
                return Err(AppError::BadRequest(
                    "the catalog does not allow that mode for the image".into(),
                ));
            }
            bli_kvm_core::VirtualMediaCommand::Mount {
                image_id: entry.image_id,
                mode,
                writable: entry.writable_allowed,
            }
        };
        Ok(Json(ApiResponse::ok(
            apply_media_command(
                &state,
                &session.token,
                command,
                b"hardened-blikvm-media-mount-v1",
            )
            .await?,
        )))
    }
    #[cfg(not(feature = "restricted-control-surface"))]
    {
        let _ = (&state, &session, &body);
        Err(AppError::ServiceUnavailable(
            "virtual media control is not available".into(),
        ))
    }
}

/// Re-enumerate the USB gadget without changing the selection.
pub(crate) async fn storage_reconnect(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<StorageOutcome>>> {
    #[cfg(feature = "restricted-control-surface")]
    {
        Ok(Json(ApiResponse::ok(
            apply_media_command(
                &state,
                &session.token,
                bli_kvm_core::VirtualMediaCommand::Reconnect,
                b"hardened-blikvm-media-reconnect-v1",
            )
            .await?,
        )))
    }
    #[cfg(not(feature = "restricted-control-surface"))]
    {
        let _ = (&state, &session);
        Err(AppError::ServiceUnavailable(
            "virtual media control is not available".into(),
        ))
    }
}

/// Delete one image; refuses while it is the mounted image.
pub(crate) async fn storage_delete(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<StorageDeleteBody>,
) -> AppResult<Json<ApiResponse<StorageOutcome>>> {
    #[cfg(feature = "restricted-control-surface")]
    {
        if body.file.is_empty() || body.file.len() > 128 || body.file.contains('/') {
            return Err(AppError::BadRequest("image name is invalid".into()));
        }
        let observed = observe_media(&state, &session.token).await?;
        if observed.file.as_deref() == Some(body.file.as_str()) {
            return Err(AppError::Conflict(
                "the mounted image cannot be deleted".into(),
            ));
        }
        let catalog_path = storage_catalog_path(&state)?.to_path_buf();
        bli_kvm_media_catalog::MediaCatalog::remove_image(&catalog_path, &body.file).map_err(
            |error| match error {
                bli_kvm_media_catalog::MediaCatalogError::NotFound => {
                    AppError::BadRequest("image is not present in the catalog".into())
                }
                _ => AppError::ServiceUnavailable("virtual media catalog is unavailable".into()),
            },
        )?;
        if let Some(directory) = state.options.media_directory.as_deref() {
            let path: PathBuf = directory.join(&body.file);
            if path.is_file() {
                std::fs::remove_file(&path).map_err(|_| {
                    AppError::ServiceUnavailable("the image file could not be removed".into())
                })?;
            }
        }
        Ok(Json(ApiResponse::ok(StorageOutcome {
            applied: true,
            rejection: None,
        })))
    }
    #[cfg(not(feature = "restricted-control-surface"))]
    {
        let _ = (&state, &session, &body);
        Err(AppError::ServiceUnavailable(
            "virtual media control is not available".into(),
        ))
    }
}
