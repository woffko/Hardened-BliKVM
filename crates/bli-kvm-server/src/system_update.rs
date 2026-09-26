//! RAW system update (`docs/design/system-raw-update-v1.md`), switched off by
//! default. The reference's paths where the meaning matches
//! (`/api/system-update/raw-enabled`, `status`, `install`, `confirm`,
//! `rollback`) plus an upload, because the bundle comes from the operator
//! rather than a release feed.
//!
//! The server checks the switch and forwards; the root administration helper
//! re-checks it, verifies the signed bundle and stages it, and the boot-time
//! step writes it. Only the status and cancelling a staged update work while
//! the switch is off.

use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

use axum::{
    Extension, Json,
    extract::{Request, State},
    http::{HeaderMap, header},
};
use bli_kvm_admin_helper::{
    AdminReply, AdminRequest, FRAME_DATA, MAX_FRAME_BYTES, MAX_SYSTEM_BUNDLE_BYTES,
    SystemUpdateState, encode_frame,
};
use bli_kvm_core::AdminFeature;
use http_body::Body as _;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::{
    AppState,
    admin_features::EnabledBody,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
    system_action::SystemActionOutcome,
};

/// Verifying a bundle and archiving the device state can take minutes.
const STAGE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_REPLY_LINE: u64 = 64 * 1024;

#[derive(Debug, Serialize)]
pub(crate) struct EnabledView {
    enabled: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfirmBody {
    #[serde(default)]
    confirm: bool,
}

fn helper(state: &AppState) -> AppResult<&Path> {
    state.options.admin_socket.as_deref().ok_or_else(|| {
        AppError::ServiceUnavailable("the administration helper is not configured".into())
    })
}

fn answer(reply: AdminReply) -> AppResult<SystemUpdateState> {
    match reply {
        AdminReply::SystemUpdate { update } => Ok(update),
        AdminReply::Refused { reason } => Err(AppError::Conflict(reason)),
        _ => Err(AppError::ServiceUnavailable(
            "unexpected helper answer".into(),
        )),
    }
}

async fn call(state: &AppState, request: AdminRequest) -> AppResult<SystemUpdateState> {
    answer(crate::scripts::call(helper(state)?, &request, None).await?)
}

/// Reference: `GET /api/system-update/raw-enabled`.
pub(crate) async fn raw_enabled() -> Json<ApiResponse<EnabledView>> {
    Json(ApiResponse::ok(EnabledView {
        enabled: crate::admin_features::switches().get(AdminFeature::RawSystemUpdate),
    }))
}

/// Reference: `POST /api/system-update/raw-enabled`. Switching off drops an
/// update that is staged but not written yet.
pub(crate) async fn set_raw_enabled(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<EnabledBody>,
) -> AppResult<Json<ApiResponse<crate::admin_features::EnabledView>>> {
    let result = crate::admin_features::set(
        &state,
        &session.token,
        AdminFeature::RawSystemUpdate,
        body.enabled,
    )
    .await?;
    if !crate::admin_features::switches().get(AdminFeature::RawSystemUpdate)
        && let Ok(update) = call(&state, AdminRequest::SystemUpdateStatus {}).await
        && update.phase == "staged"
    {
        let _ = call(&state, AdminRequest::SystemUpdateCancel {}).await;
    }
    Ok(result)
}

/// Reference: `GET /api/system-update/status`.
pub(crate) async fn status(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<SystemUpdateState>>> {
    Ok(Json(ApiResponse::ok(
        call(&state, AdminRequest::SystemUpdateStatus {}).await?,
    )))
}

fn upload_length(headers: &HeaderMap) -> AppResult<u64> {
    let invalid = || {
        AppError::BadRequest(
            "the upload needs an unencoded application/octet-stream body with a length".into(),
        )
    };
    if headers.contains_key(header::CONTENT_ENCODING)
        || headers.contains_key(header::TRANSFER_ENCODING)
        || headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            != Some("application/octet-stream")
        || headers.get_all(header::CONTENT_LENGTH).iter().count() != 1
    {
        return Err(invalid());
    }
    let length: u64 = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|v| v.parse().ok())
        .ok_or_else(invalid)?;
    if !(1..=MAX_SYSTEM_BUNDLE_BYTES).contains(&length) {
        return Err(AppError::BadRequest(
            "the system bundle is too large".into(),
        ));
    }
    Ok(length)
}

async fn read_reply(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
) -> AppResult<AdminReply> {
    let unavailable =
        || AppError::ServiceUnavailable("the administration helper is unavailable".into());
    let mut line = Vec::new();
    (&mut *reader)
        .take(MAX_REPLY_LINE)
        .read_until(b'\n', &mut line)
        .await
        .map_err(|_| unavailable())?;
    serde_json::from_slice(line.strip_suffix(b"\n").unwrap_or(&line)).map_err(|_| unavailable())
}

/// Streams a signed system bundle to the helper, which verifies and stages
/// it for the next boot.
pub(crate) async fn upload(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    request: Request,
) -> AppResult<Json<ApiResponse<SystemUpdateState>>> {
    crate::admin_features::require(AdminFeature::RawSystemUpdate)?;
    let socket = helper(&state)?;
    let length = upload_length(request.headers())?;
    let unavailable =
        || AppError::ServiceUnavailable("the administration helper is unavailable".into());
    let stream = tokio::net::UnixStream::connect(socket)
        .await
        .map_err(|_| unavailable())?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = serde_json::to_vec(&AdminRequest::SystemUpdateUpload { size: length })
        .map_err(|_| unavailable())?;
    line.push(b'\n');
    writer.write_all(&line).await.map_err(|_| unavailable())?;
    match tokio::time::timeout(Duration::from_secs(30), read_reply(&mut reader))
        .await
        .map_err(|_| unavailable())??
    {
        AdminReply::Ready => {}
        reply => return Err(answer(reply).err().unwrap_or_else(unavailable)),
    }
    let mut body = request.into_body();
    let mut sent = 0_u64;
    while let Some(frame) = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
        if !session.is_active() {
            return Err(AppError::Unauthorized);
        }
        let frame = frame.map_err(|_| AppError::BadRequest("invalid upload body".into()))?;
        let Ok(data) = frame.into_data() else {
            return Err(AppError::BadRequest(
                "upload trailers are not accepted".into(),
            ));
        };
        sent += data.len() as u64;
        if sent > length {
            return Err(AppError::BadRequest("upload body length mismatch".into()));
        }
        for chunk in data.chunks(MAX_FRAME_BYTES) {
            let frame = encode_frame(FRAME_DATA, chunk).map_err(|_| unavailable())?;
            writer.write_all(&frame).await.map_err(|_| unavailable())?;
        }
    }
    if sent != length {
        return Err(AppError::BadRequest("upload body length mismatch".into()));
    }
    let reply = tokio::time::timeout(STAGE_TIMEOUT, read_reply(&mut reader))
        .await
        .map_err(|_| {
            AppError::ServiceUnavailable("staging the system update timed out".into())
        })??;
    Ok(Json(ApiResponse::ok(answer(reply)?)))
}

/// Reference: `POST /api/system-update/install`: reboots into the boot-time
/// step that writes the staged update.
pub(crate) async fn install(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<ConfirmBody>,
) -> AppResult<Json<ApiResponse<SystemActionOutcome>>> {
    crate::admin_features::require(AdminFeature::RawSystemUpdate)?;
    if !body.confirm {
        return Err(AppError::BadRequest(
            "installing needs an explicit confirmation".into(),
        ));
    }
    let update = call(&state, AdminRequest::SystemUpdateStatus {}).await?;
    if update.phase != "staged" {
        return Err(AppError::Conflict("no system update is staged".into()));
    }
    crate::system_action::confirmed_reboot(state, session).await
}

/// Reference: `POST /api/system-update/confirm`.
pub(crate) async fn confirm(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<SystemUpdateState>>> {
    crate::admin_features::require(AdminFeature::RawSystemUpdate)?;
    Ok(Json(ApiResponse::ok(
        call(&state, AdminRequest::SystemUpdateConfirm {}).await?,
    )))
}

/// Reference: `POST /api/system-update/rollback`: the next boot restores the
/// previous system, and the board reboots now.
pub(crate) async fn rollback(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<ConfirmBody>,
) -> AppResult<Json<ApiResponse<SystemActionOutcome>>> {
    crate::admin_features::require(AdminFeature::RawSystemUpdate)?;
    if !body.confirm {
        return Err(AppError::BadRequest(
            "rolling back needs an explicit confirmation".into(),
        ));
    }
    call(&state, AdminRequest::SystemUpdateRollback {}).await?;
    crate::system_action::confirmed_reboot(state, session).await
}

/// Drops a staged update that has not been written.
pub(crate) async fn cancel(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<SystemUpdateState>>> {
    Ok(Json(ApiResponse::ok(
        call(&state, AdminRequest::SystemUpdateCancel {}).await?,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uploads_need_an_exact_octet_stream_length() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            "application/octet-stream".parse().unwrap(),
        );
        headers.insert(header::CONTENT_LENGTH, "1048576".parse().unwrap());
        assert_eq!(upload_length(&headers).unwrap(), 1_048_576);
        for (name, value) in [
            (header::CONTENT_LENGTH, "0"),
            (header::CONTENT_LENGTH, "+5"),
            (header::CONTENT_LENGTH, "9000000000000"),
            (header::CONTENT_TYPE, "application/x-tar"),
        ] {
            let mut bad = headers.clone();
            bad.insert(name, value.parse().unwrap());
            assert!(upload_length(&bad).is_err(), "{value}");
        }
        let mut encoded = headers.clone();
        encoded.insert(header::CONTENT_ENCODING, "gzip".parse().unwrap());
        assert!(upload_length(&encoded).is_err());
    }
}
