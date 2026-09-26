//! Stored scripts (reference `/api/vm/script*`), switched off by default.
//!
//! The server validates and forwards; the root administration helper stores
//! and runs the scripts and refuses on its own while the switch is off.

use std::path::Path;
use std::time::Duration;

use axum::{
    Json,
    extract::{Multipart, State},
};
use bli_kvm_admin_helper::{
    AdminReply, AdminRequest, FRAME_DATA, MAX_FRAME_BYTES, MAX_LINE_BYTES, MAX_SCRIPT_BYTES,
    encode_frame, valid_script_name,
};
use bli_kvm_core::AdminFeature;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{
    AppState,
    error::{ApiResponse, AppError, AppResult},
};

/// A foreground run may take the helper's full minute.
const CALL_TIMEOUT: Duration = Duration::from_secs(75);

#[derive(Debug, Serialize)]
pub(crate) struct ScriptList {
    files: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct UploadedScript {
    file: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScriptName {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunBody {
    name: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct RunLog {
    log: String,
}

fn helper(state: &AppState) -> AppResult<&Path> {
    crate::admin_features::require(AdminFeature::Scripts)?;
    state
        .options
        .admin_socket
        .as_deref()
        .ok_or_else(|| AppError::ServiceUnavailable("the script helper is not configured".into()))
}

fn checked_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if valid_script_name(name) {
        Ok(name.to_string())
    } else {
        Err(AppError::BadRequest("invalid script filename".into()))
    }
}

/// One request to the administration helper, with optional upload data.
pub(crate) async fn call(
    socket: &Path,
    request: &AdminRequest,
    upload: Option<&[u8]>,
) -> AppResult<AdminReply> {
    let unavailable =
        || AppError::ServiceUnavailable("the administration helper is unavailable".into());
    tokio::time::timeout(CALL_TIMEOUT, async {
        let mut stream = tokio::net::UnixStream::connect(socket)
            .await
            .map_err(|_| unavailable())?;
        let mut line = serde_json::to_vec(request).map_err(|_| unavailable())?;
        line.push(b'\n');
        stream.write_all(&line).await.map_err(|_| unavailable())?;
        if let Some(data) = upload {
            for chunk in data.chunks(MAX_FRAME_BYTES) {
                let frame = encode_frame(FRAME_DATA, chunk).map_err(|_| unavailable())?;
                stream.write_all(&frame).await.map_err(|_| unavailable())?;
            }
        }
        let mut reply = Vec::new();
        stream
            .take(MAX_LINE_BYTES as u64 * 32)
            .read_to_end(&mut reply)
            .await
            .map_err(|_| unavailable())?;
        let line = reply
            .split(|byte| *byte == b'\n')
            .next()
            .unwrap_or_default();
        serde_json::from_slice::<AdminReply>(line).map_err(|_| unavailable())
    })
    .await
    .map_err(|_| AppError::ServiceUnavailable("the administration helper timed out".into()))?
}

fn refused(reply: AdminReply) -> AppError {
    match reply {
        AdminReply::Refused { reason } => AppError::BadRequest(reason),
        _ => AppError::ServiceUnavailable("unexpected helper answer".into()),
    }
}

pub(crate) async fn list(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<ScriptList>>> {
    let socket = helper(&state)?;
    match call(socket, &AdminRequest::ScriptList {}, None).await? {
        AdminReply::Scripts { files } => Ok(Json(ApiResponse::ok(ScriptList { files }))),
        other => Err(refused(other)),
    }
}

pub(crate) async fn upload(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> AppResult<Json<ApiResponse<UploadedScript>>> {
    let socket = helper(&state)?;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| AppError::BadRequest("invalid multipart script upload".into()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let name = checked_name(
            field
                .file_name()
                .ok_or_else(|| AppError::BadRequest("missing script filename".into()))?,
        )?;
        let data = field
            .bytes()
            .await
            .map_err(|_| AppError::BadRequest("read script upload failed".into()))?;
        if data.is_empty() || data.len() > MAX_SCRIPT_BYTES {
            return Err(AppError::BadRequest("script size out of bounds".into()));
        }
        let request = AdminRequest::ScriptUpload {
            name: name.clone(),
            size: data.len() as u32,
        };
        return match call(socket, &request, Some(&data)).await? {
            AdminReply::Done => Ok(Json(ApiResponse::ok(UploadedScript { file: name }))),
            other => Err(refused(other)),
        };
    }
    Err(AppError::BadRequest("missing script file".into()))
}

pub(crate) async fn delete(
    State(state): State<AppState>,
    Json(body): Json<ScriptName>,
) -> AppResult<Json<ApiResponse<()>>> {
    let socket = helper(&state)?;
    let request = AdminRequest::ScriptDelete {
        name: checked_name(&body.name)?,
    };
    match call(socket, &request, None).await? {
        AdminReply::Done => Ok(Json(ApiResponse::ok(()))),
        other => Err(refused(other)),
    }
}

pub(crate) async fn run(
    State(state): State<AppState>,
    Json(body): Json<RunBody>,
) -> AppResult<Json<ApiResponse<RunLog>>> {
    let socket = helper(&state)?;
    let background = match body.kind.as_str() {
        "foreground" => false,
        "background" => true,
        _ => return Err(AppError::BadRequest("invalid script run type".into())),
    };
    let request = AdminRequest::ScriptRun {
        name: checked_name(&body.name)?,
        background,
    };
    match call(socket, &request, None).await? {
        AdminReply::Output { log, success: true } => Ok(Json(ApiResponse::ok(RunLog { log }))),
        // The reference reports a failed run as an error.
        AdminReply::Output { success: false, .. } => {
            Err(AppError::BadRequest("run script failed".into()))
        }
        AdminReply::Done => Ok(Json(ApiResponse::ok(RunLog { log: String::new() }))),
        other => Err(refused(other)),
    }
}
