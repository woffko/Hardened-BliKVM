//! Authenticated four-mode firewall API; native configuration remains owner-only.
use crate::{
    AppState,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
};
use axum::{Extension, Json, extract::State};
use bli_kvm_core::FirewallMode;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ChangeBody {
    mode: FirewallMode,
    #[serde(default)]
    confirm: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConfirmBody {
    operation_id: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Outcome {
    applied: bool,
    rejection: Option<&'static str>,
    operation_id: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    mode: FirewallMode,
    pending_operation_id: Option<String>,
    confirmation_deadline_unix_ms: Option<String>,
    recovery_required: bool,
    configuration_ready: bool,
}

fn parse_operation(value: &str) -> Option<[u8; 16]> {
    if value.len() != 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut bytes = [0; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    (bytes != [0; 16]).then_some(bytes)
}

#[cfg(feature = "restricted-control-surface")]
fn hex(value: &[u8]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}
#[cfg(feature = "restricted-control-surface")]
fn identity(session: &crate::auth::session::Session) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"hardened-blikvm-firewall-session-v1");
    hasher.update(session.token.as_bytes());
    hasher.finalize().into()
}
#[cfg(feature = "restricted-control-surface")]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}
#[cfg(feature = "restricted-control-surface")]
async fn execute(
    state: &AppState,
    request: bli_kvm_core::PlatformControlRequestV2,
) -> AppResult<bli_kvm_control_protocol::ControlResponse> {
    use rand_core::RngCore;
    if !state.options.restricted_controls_enabled || !state.options.firewall_modes_enabled {
        return Err(AppError::ServiceUnavailable(
            "firewall controls are unavailable".into(),
        ));
    }
    let control = state
        .control
        .as_ref()
        .ok_or_else(|| AppError::ServiceUnavailable("firewall owner is unavailable".into()))?;
    let mut operation = [0; 16];
    rand_core::OsRng
        .try_fill_bytes(&mut operation)
        .map_err(|_| AppError::ServiceUnavailable("operation identity unavailable".into()))?;
    control
        .execute_v2(hex(&operation), request)
        .await
        .map_err(|_| AppError::ServiceUnavailable("firewall owner is unavailable".into()))
}

pub(crate) async fn status(
    State(state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<Status>>> {
    #[cfg(feature = "restricted-control-surface")]
    {
        if let bli_kvm_control_protocol::ControlResponse::FirewallModeState { status } = execute(
            &state,
            bli_kvm_core::PlatformControlRequestV2::FirewallModeStatus {},
        )
        .await?
        {
            return Ok(Json(ApiResponse::ok(Status {
                mode: status.mode,
                pending_operation_id: status.pending_operation_id.as_ref().map(|id| hex(id)),
                confirmation_deadline_unix_ms: status
                    .confirmation_deadline_unix_ms
                    .map(|deadline| deadline.to_string()),
                recovery_required: status.recovery_required,
                configuration_ready: status.configuration_ready,
            })));
        }
    }
    #[cfg(not(feature = "restricted-control-surface"))]
    let _ = state;
    Err(AppError::ServiceUnavailable(
        "firewall modes are unavailable".into(),
    ))
}

pub(crate) async fn change(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<ChangeBody>,
) -> AppResult<Json<ApiResponse<Outcome>>> {
    if !body.confirm {
        return Err(AppError::BadRequest(
            "explicit firewall confirmation is required".into(),
        ));
    }
    #[cfg(feature = "restricted-control-surface")]
    {
        use rand_core::RngCore;
        let mut operation = [0; 16];
        rand_core::OsRng
            .try_fill_bytes(&mut operation)
            .map_err(|_| AppError::ServiceUnavailable("operation identity unavailable".into()))?;
        let request = bli_kvm_core::FirewallChangeRequest {
            operation_id: operation,
            session_binding: identity(&session),
            mode: body.mode,
            issued_at_unix_ms: now_ms(),
        };
        let response = execute(
            &state,
            bli_kvm_core::PlatformControlRequestV2::FirewallModeChange { request },
        )
        .await?;
        Ok(Json(ApiResponse::ok(outcome(response, hex(&operation)))))
    }
    #[cfg(not(feature = "restricted-control-surface"))]
    {
        let _ = (state, session, body.mode);
        Err(AppError::ServiceUnavailable(
            "firewall modes are unavailable".into(),
        ))
    }
}

pub(crate) async fn confirm(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<ConfirmBody>,
) -> AppResult<Json<ApiResponse<Outcome>>> {
    let operation = parse_operation(&body.operation_id)
        .ok_or_else(|| AppError::BadRequest("invalid firewall operation ID".into()))?;
    #[cfg(feature = "restricted-control-surface")]
    {
        let request = bli_kvm_core::FirewallConfirmRequest {
            operation_id: operation,
            session_binding: identity(&session),
            issued_at_unix_ms: now_ms(),
        };
        let response = execute(
            &state,
            bli_kvm_core::PlatformControlRequestV2::FirewallModeConfirm { request },
        )
        .await?;
        Ok(Json(ApiResponse::ok(outcome(response, body.operation_id))))
    }
    #[cfg(not(feature = "restricted-control-surface"))]
    {
        let _ = (state, session, operation);
        Err(AppError::ServiceUnavailable(
            "firewall modes are unavailable".into(),
        ))
    }
}

#[cfg(feature = "restricted-control-surface")]
fn outcome(response: bli_kvm_control_protocol::ControlResponse, operation_id: String) -> Outcome {
    use bli_kvm_control_protocol::ControlResponse;
    match response {
        ControlResponse::Applied => Outcome {
            applied: true,
            rejection: None,
            operation_id,
        },
        ControlResponse::Rejected { code } => Outcome {
            applied: false,
            rejection: Some(crate::hid::rejection_name(code)),
            operation_id,
        },
        _ => Outcome {
            applied: false,
            rejection: Some("unexpected_response"),
            operation_id,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confirmation_ids_are_exact_nonzero_lowercase_hex() {
        assert_eq!(parse_operation(&"ab".repeat(16)), Some([0xab; 16]));
        for invalid in [
            "00".repeat(16),
            "AB".repeat(16),
            "ab".repeat(15),
            "é".repeat(16),
        ] {
            assert!(parse_operation(&invalid).is_none());
        }
    }
}
