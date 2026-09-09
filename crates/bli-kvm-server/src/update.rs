use axum::{
    Extension, Json,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, header},
};
use bli_kvm_update_client::{ActivationClientError, UploadClientError};
use bli_kvm_update_protocol::activation::{
    self, ActivationErrorCode, ActivationReceipt, ActivationRequest, ActivationRequestEnvelope,
};
use bli_kvm_update_protocol::upload::{
    MAX_UPLOAD_ARCHIVE_BYTES, UploadDisposition, UploadErrorCode, UploadReceipt,
};
use bli_kvm_update_protocol::{
    ApplicationRollbackReason, ApplicationSlotVersion, ApplicationUpdateStatus,
    PendingApplicationUpdate, UPDATE_SCHEMA_VERSION, UpdatePendingPhase,
};
use bytes::Bytes;
use http_body::Body as _;
use serde::{Deserialize, Serialize};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

use crate::{
    AppState,
    auth::{middleware::CurrentSession, session::Session},
    error::{ApiResponse, AppError, AppResult},
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SlotView {
    slot_id: String,
    version: String,
    sequence: String,
    minimum_system_sequence: String,
}

impl From<ApplicationSlotVersion> for SlotView {
    fn from(slot: ApplicationSlotVersion) -> Self {
        Self {
            slot_id: slot.slot_id,
            version: slot.version,
            sequence: slot.sequence.to_string(),
            minimum_system_sequence: slot.minimum_system_sequence.to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingView {
    operation_id: String,
    target: SlotView,
    phase: UpdatePendingPhase,
}

impl From<PendingApplicationUpdate> for PendingView {
    fn from(pending: PendingApplicationUpdate) -> Self {
        Self {
            operation_id: pending.operation_id,
            target: pending.target.into(),
            phase: pending.phase,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateStatusView {
    schema_version: u16,
    // Decimal strings preserve exact u64 values through JavaScript clients.
    generation: String,
    floor_sequence: String,
    system_sequence: String,
    selected: SlotView,
    confirmed: SlotView,
    previous: Option<SlotView>,
    pending: Option<PendingView>,
    last_rollback: Option<ApplicationRollbackReason>,
}

impl From<ApplicationUpdateStatus> for UpdateStatusView {
    fn from(status: ApplicationUpdateStatus) -> Self {
        Self {
            schema_version: UPDATE_SCHEMA_VERSION,
            generation: status.generation.to_string(),
            floor_sequence: status.floor_sequence.to_string(),
            system_sequence: status.system_sequence.to_string(),
            selected: status.selected.into(),
            confirmed: status.confirmed.into(),
            previous: status.previous.map(Into::into),
            pending: status.pending.map(Into::into),
            last_rollback: status.last_rollback,
        }
    }
}

pub(crate) async fn status(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<UpdateStatusView>>> {
    let client = state.update_status.as_ref().ok_or_else(|| {
        AppError::ServiceUnavailable("application update status is not configured".into())
    })?;
    let status = client.status().await.map_err(|_| {
        AppError::ServiceUnavailable("application update status is unavailable".into())
    })?;
    Ok(Json(ApiResponse::ok(status.into())))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActivationBody {
    slot_id: String,
    expected_generation: String,
}

fn activation_body(bytes: &[u8]) -> AppResult<(String, u64)> {
    let invalid = || AppError::BadRequest("invalid activation request".into());
    let body: ActivationBody = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let value = &body.expected_generation;
    if value.is_empty() || value.starts_with('0') || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let generation: u64 = value.parse().map_err(|_| invalid())?;
    activation::encode_request(&ActivationRequestEnvelope {
        schema_version: UPDATE_SCHEMA_VERSION,
        request_id: 1,
        request: ActivationRequest::Activate {
            slot_id: body.slot_id.clone(),
            expected_generation: generation,
        },
    })
    .map_err(|_| invalid())?;
    Ok((body.slot_id, generation))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActivationView {
    phase: &'static str,
    receipt: ActivationReceipt,
}

pub(crate) async fn activate(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    request: Request,
) -> AppResult<(axum::http::StatusCode, Json<ApiResponse<ActivationView>>)> {
    // Protected middleware has checked authentication, origin and CSRF first.
    let client = state
        .update_activation
        .as_ref()
        .ok_or_else(|| AppError::ServiceUnavailable("activation is not configured".into()))?;
    if request
        .headers()
        .get_all(header::CONTENT_TYPE)
        .iter()
        .count()
        != 1
        || request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            != Some("application/json")
        || request.headers().contains_key(header::CONTENT_ENCODING)
        || request.headers().contains_key(header::TRAILER)
    {
        return Err(AppError::BadRequest(
            "activation requires unencoded application/json".into(),
        ));
    }
    // Reuse the session-aware frame adapter so trailers and body errors cannot
    // be accepted, with a small absolute allocation/read-time bound.
    let reader = UploadBody {
        body: request.into_body(),
        chunk: Bytes::new(),
        session: session.clone(),
    };
    let mut bytes = Vec::new();
    let limit = bli_kvm_update_protocol::MAX_UPDATE_MESSAGE_BYTES;
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::io::AsyncReadExt::read_to_end(
            &mut tokio::io::AsyncReadExt::take(reader, (limit + 1) as u64),
            &mut bytes,
        ),
    )
    .await
    .map_err(|_| AppError::BadRequest("activation body timed out".into()))?
    .map_err(|_| AppError::BadRequest("invalid activation body".into()))?;
    if bytes.len() > limit {
        return Err(AppError::BadRequest("activation body too large".into()));
    }
    let (slot_id, generation) = activation_body(&bytes)?;
    if !session.is_active() {
        return Err(AppError::Unauthorized);
    }
    // No cancellation implies rollback, and no failure authorizes a replay.
    let receipt = client
        .activate(&slot_id, generation)
        .await
        .map_err(|error| match error {
            ActivationClientError::Busy
            | ActivationClientError::Rejected(ActivationErrorCode::Busy) => {
                AppError::Conflict("another activation is in progress".into())
            }
            ActivationClientError::Rejected(ActivationErrorCode::StaleGeneration) => {
                AppError::Conflict("update state changed; refresh and confirm again".into())
            }
            ActivationClientError::Rejected(ActivationErrorCode::AlreadyActive) => {
                AppError::Conflict("target is already active".into())
            }
            ActivationClientError::Rejected(ActivationErrorCode::PendingActivation) => {
                AppError::Conflict("an activation is awaiting health".into())
            }
            ActivationClientError::Rejected(ActivationErrorCode::InvalidTarget) => {
                AppError::BadRequest("activation target is not eligible".into())
            }
            _ => {
                AppError::ServiceUnavailable("activation outcome unknown; reconcile status".into())
            }
        })?;
    if !session.is_active() {
        return Err(AppError::Unauthorized);
    }
    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(ApiResponse::ok(ActivationView {
            phase: "awaiting_health",
            receipt,
        })),
    ))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UploadView {
    slot: SlotView,
    disposition: UploadDisposition,
    manifest_sha256: String,
    payload_bytes: String,
}

impl From<UploadReceipt> for UploadView {
    fn from(receipt: UploadReceipt) -> Self {
        Self {
            slot: receipt.slot.into(),
            disposition: receipt.disposition,
            manifest_sha256: receipt.manifest_sha256,
            payload_bytes: receipt.payload_bytes.to_string(),
        }
    }
}

fn upload_length(headers: &HeaderMap) -> AppResult<u64> {
    let invalid = || {
        AppError::BadRequest("upload requires an exact unencoded application/octet-stream body of 1 to 134217728 bytes".into())
    };
    for name in [
        header::CONTENT_ENCODING,
        header::TRANSFER_ENCODING,
        header::TRAILER,
    ] {
        if headers.contains_key(name) {
            return Err(invalid());
        }
    }
    if headers.get_all(header::CONTENT_TYPE).iter().count() != 1
        || headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            != Some("application/octet-stream")
        || headers.get_all(header::CONTENT_LENGTH).iter().count() != 1
    {
        return Err(invalid());
    }
    let value = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(invalid)?;
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let length: u64 = value.parse().map_err(|_| invalid())?;
    if !(1..=MAX_UPLOAD_ARCHIVE_BYTES).contains(&length) {
        return Err(invalid());
    }
    Ok(length)
}

// Backpressure adapter: retains only the current HTTP frame, never collects
// the archive. Trailers are invalid even if all data bytes have arrived.
struct UploadBody {
    body: Body,
    chunk: Bytes,
    session: Session,
}
impl AsyncRead for UploadBody {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if output.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        for _ in 0..32 {
            if !self.session.is_active() {
                return Poll::Ready(Err(std::io::Error::other("upload session ended")));
            }
            if !self.chunk.is_empty() {
                let count = output.remaining().min(self.chunk.len());
                output.put_slice(&self.chunk.split_to(count));
                return Poll::Ready(Ok(()));
            }
            match std::task::ready!(Pin::new(&mut self.body).poll_frame(cx)) {
                None => return Poll::Ready(Ok(())),
                Some(Err(_)) => {
                    return Poll::Ready(Err(std::io::Error::other("invalid upload body")));
                }
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(bytes) => self.chunk = bytes,
                    Err(_) => {
                        return Poll::Ready(Err(std::io::Error::other(
                            "upload trailers forbidden",
                        )));
                    }
                },
            }
        }
        // Bound work per poll even for an adversarial stream of empty frames.
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

pub(crate) async fn upload(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    request: Request,
) -> AppResult<Json<ApiResponse<UploadView>>> {
    // Authentication and existing unsafe-method origin policy ran in the
    // protected route middleware, before this body or IPC can be touched.
    let length = upload_length(request.headers())?;
    let client = state
        .update_upload
        .as_ref()
        .ok_or_else(|| AppError::ServiceUnavailable("upload is not configured".into()))?;
    let reader = UploadBody {
        body: request.into_body(),
        chunk: Bytes::new(),
        session: session.clone(),
    };
    let result = tokio::select! {
        biased;
        _ = async {
            while session.is_active() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        } => return Err(AppError::Unauthorized),
        result = client.stage(length, reader) => result,
    };
    if !session.is_active() {
        return Err(AppError::Unauthorized);
    }
    let receipt = result.map_err(|error| match error {
        UploadClientError::LengthMismatch => {
            AppError::BadRequest("upload body length mismatch".into())
        }
        UploadClientError::Rejected(UploadErrorCode::InvalidArchive) => {
            AppError::BadRequest("archive verification failed".into())
        }
        UploadClientError::Busy | UploadClientError::Rejected(UploadErrorCode::Busy) => {
            AppError::Conflict("another upload is in progress".into())
        }
        UploadClientError::Rejected(UploadErrorCode::QuotaExceeded) => {
            AppError::Conflict("upload staging quota exceeded".into())
        }
        _ => AppError::ServiceUnavailable(
            "upload result unavailable; retry the same archive to reconcile".into(),
        ),
    })?;
    Ok(Json(ApiResponse::ok(receipt.into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_accepts_only_exact_decimal_generation_and_closed_intent() {
        let slot = format!("app-00000000000000000002-{}", "a".repeat(64));
        let valid = serde_json::json!({"slotId": slot, "expectedGeneration": u64::MAX.to_string()});
        assert_eq!(
            activation_body(&serde_json::to_vec(&valid).unwrap()).unwrap(),
            (slot, u64::MAX)
        );
        for value in [
            serde_json::json!(1),
            serde_json::json!(null),
            serde_json::json!("0"),
            serde_json::json!("01"),
            serde_json::json!("+1"),
            serde_json::json!(" 1"),
            serde_json::json!("1.0"),
            serde_json::json!("18446744073709551616"),
        ] {
            let mut body = valid.clone();
            body["expectedGeneration"] = value;
            assert!(activation_body(&serde_json::to_vec(&body).unwrap()).is_err());
        }
        for field in ["command", "url", "healthToken", "confirm"] {
            let mut body = valid.clone();
            body[field] = serde_json::json!("forbidden");
            assert!(activation_body(&serde_json::to_vec(&body).unwrap()).is_err());
        }
        assert!(activation_body(br#"{"slotId":"../bad","expectedGeneration":"1"}"#).is_err());
    }

    fn raw_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            "application/octet-stream".parse().unwrap(),
        );
        headers.insert(header::CONTENT_LENGTH, "3".parse().unwrap());
        headers
    }

    #[test]
    fn strict_upload_header_contract() {
        for length in ["1", "134217728"] {
            let mut headers = raw_headers();
            headers.insert(header::CONTENT_LENGTH, length.parse().unwrap());
            assert_eq!(
                upload_length(&headers).unwrap(),
                length.parse::<u64>().unwrap()
            );
        }
        for length in [
            "",
            "0",
            "134217729",
            "18446744073709551616",
            "-1",
            "+1",
            "3,3",
            " 3",
        ] {
            let mut headers = raw_headers();
            headers.insert(header::CONTENT_LENGTH, length.parse().unwrap());
            assert!(upload_length(&headers).is_err());
        }
        for name in [header::CONTENT_LENGTH, header::CONTENT_TYPE] {
            let mut headers = raw_headers();
            headers.remove(&name);
            assert!(upload_length(&headers).is_err());
            headers = raw_headers();
            headers.append(name.clone(), headers[&name].clone());
            assert!(upload_length(&headers).is_err());
        }
        for name in [
            header::CONTENT_ENCODING,
            header::TRANSFER_ENCODING,
            header::TRAILER,
        ] {
            let mut headers = raw_headers();
            headers.insert(name, "identity".parse().unwrap());
            assert!(upload_length(&headers).is_err());
        }
        for kind in [
            "multipart/form-data",
            "application/octet-stream; charset=utf-8",
            "application/json",
        ] {
            let mut headers = raw_headers();
            headers.insert(header::CONTENT_TYPE, kind.parse().unwrap());
            assert!(upload_length(&headers).is_err());
        }
    }

    #[tokio::test]
    async fn adapter_rejects_trailers_body_errors_and_revoked_sessions() {
        struct BadBody(bool);
        impl http_body::Body for BadBody {
            type Data = Bytes;
            type Error = std::io::Error;
            fn poll_frame(
                self: Pin<&mut Self>,
                _: &mut Context<'_>,
            ) -> Poll<Option<Result<http_body::Frame<Bytes>, Self::Error>>> {
                Poll::Ready(Some(if self.0 {
                    Ok(http_body::Frame::trailers(HeaderMap::new()))
                } else {
                    Err(std::io::Error::other("fixture failure"))
                }))
            }
        }
        use tokio::io::AsyncReadExt;
        let sessions = crate::auth::session::SessionStore::new();
        let session = sessions.issue("operator", 60).await;
        for trailers in [false, true] {
            let mut reader = UploadBody {
                body: Body::new(BadBody(trailers)),
                chunk: Bytes::new(),
                session: session.clone(),
            };
            assert!(reader.read(&mut [0; 1]).await.is_err());
        }
        sessions.revoke(&session.token).await;
        let mut reader = UploadBody {
            body: Body::from("abc"),
            chunk: Bytes::new(),
            session,
        };
        assert!(reader.read(&mut [0; 1]).await.is_err());
    }

    #[test]
    fn preserves_u64_counters_without_javascript_rounding() {
        let slot = ApplicationSlotVersion {
            slot_id: format!("app-{:020}-{}", u64::MAX, "a".repeat(64)),
            version: "1.0.0".into(),
            sequence: u64::MAX,
            minimum_system_sequence: 1,
        };
        let status = ApplicationUpdateStatus {
            generation: u64::MAX,
            floor_sequence: u64::MAX,
            system_sequence: 1,
            selected: slot.clone(),
            confirmed: slot,
            previous: None,
            pending: None,
            last_rollback: None,
        };
        let value = serde_json::to_value(UpdateStatusView::from(status)).unwrap();
        assert_eq!(value["generation"], "18446744073709551615");
        assert_eq!(value["floorSequence"], "18446744073709551615");
        assert_eq!(value["selected"]["sequence"], "18446744073709551615");
    }
}
