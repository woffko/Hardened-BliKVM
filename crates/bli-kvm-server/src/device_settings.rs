//! Device toggles forwarded to the board owner.
//!
//! The interface publishes the device name over mDNS. The board has no avahi;
//! the owner flips the reviewed `systemd-resolved` key and restarts that unit,
//! and the server only validates the session and reports what the owner holds.

use axum::{Extension, Json, extract::State};
use serde::{Deserialize, Serialize};
#[cfg(feature = "restricted-control-surface")]
use sha2::{Digest, Sha256};

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
};

#[cfg(feature = "restricted-control-surface")]
use bli_kvm_control_protocol::ControlResponse;
#[cfg(feature = "restricted-control-surface")]
use bli_kvm_core::PlatformControlRequestV2;

#[cfg(feature = "restricted-control-surface")]
use crate::hid::{rejection_name, session_identity};

#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) const MDNS_OP_DOMAIN: &[u8] = b"hardened-blikvm-mdns-op-v1";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToggleOutcome {
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MdnsView {
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) struct HidOnlyBody {
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) struct MouseJigglerBody {
    pub enabled: bool,
    pub mode: String,
}

/// The reference offers an absolute mode, which needs the pointer position the
/// target never reports; it is refused with a reason instead of being faked.
#[cfg(feature = "restricted-control-surface")]
fn jiggler_change(body: &MouseJigglerBody) -> Option<bli_kvm_core::MouseJigglerChange> {
    let mode = match body.mode.as_str() {
        "disable" => bli_kvm_core::MouseJigglerMode::Disable,
        "relative" => bli_kvm_core::MouseJigglerMode::Relative,
        _ => return None,
    };
    Some(bli_kvm_core::MouseJigglerChange::new(body.enabled, mode))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MouseJigglerView {
    pub enabled: bool,
    pub mode: &'static str,
}

#[cfg(feature = "restricted-control-surface")]
fn jiggler_view(response: ControlResponse) -> Option<MouseJigglerView> {
    match response {
        ControlResponse::MouseJigglerState { enabled, mode } => Some(MouseJigglerView {
            enabled,
            mode: match mode {
                bli_kvm_core::MouseJigglerMode::Relative => "relative",
                bli_kvm_core::MouseJigglerMode::Disable => "disable",
            },
        }),
        _ => None,
    }
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn mouse_jiggler_state(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<MouseJigglerView>>> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "device settings are not configured".into(),
        ));
    };
    let response = control
        .execute_v2(
            operation_id(&session.token),
            PlatformControlRequestV2::MouseJigglerState {},
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("device settings are unavailable".into()))?;
    jiggler_view(response)
        .map(|view| Json(ApiResponse::ok(view)))
        .ok_or_else(|| AppError::ServiceUnavailable("device settings are unavailable".into()))
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn mouse_jiggler_update(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<MouseJigglerBody>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    let Some(change) = jiggler_change(&body) else {
        return Ok(Json(ApiResponse::ok(ToggleOutcome {
            applied: false,
            rejection: Some("unsupported_mode"),
        })));
    };
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "device settings are not configured".into(),
        ));
    };
    let response = control
        .execute_v2(
            operation_id(&session.token),
            PlatformControlRequestV2::MouseJiggler { change },
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("device settings are unavailable".into()))?;
    Ok(Json(ApiResponse::ok(match response {
        ControlResponse::Applied => ToggleOutcome {
            applied: true,
            rejection: None,
        },
        ControlResponse::Rejected { code } => ToggleOutcome {
            applied: false,
            rejection: Some(rejection_name(code)),
        },
        _ => ToggleOutcome {
            applied: false,
            rejection: Some("unexpected_response"),
        },
    })))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HidOnlyView {
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TlsView {
    pub enabled: bool,
    pub subject: String,
    pub issuer: String,
    pub not_after: String,
    pub fingerprint_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) struct TlsBody {
    pub enabled: bool,
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn tls_state(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<TlsView>>> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "TLS control is not configured".into(),
        ));
    };
    let response = control
        .execute_v2(
            operation_id(&session.token),
            PlatformControlRequestV2::TlsState {},
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("TLS control is unavailable".into()))?;
    match response {
        ControlResponse::TlsState {
            enabled,
            subject,
            issuer,
            not_after,
            fingerprint_sha256,
        } => Ok(Json(ApiResponse::ok(TlsView {
            enabled,
            subject,
            issuer,
            not_after,
            fingerprint_sha256,
        }))),
        _ => Err(AppError::ServiceUnavailable(
            "TLS state is unavailable".into(),
        )),
    }
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn tls_update(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<TlsBody>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "TLS control is not configured".into(),
        ));
    };
    let response = control
        .execute_v2(
            operation_id(&session.token),
            PlatformControlRequestV2::TlsSet {
                enabled: body.enabled,
            },
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("TLS control is unavailable".into()))?;
    Ok(Json(ApiResponse::ok(match response {
        ControlResponse::Applied => ToggleOutcome {
            applied: true,
            rejection: None,
        },
        ControlResponse::Rejected { code } => ToggleOutcome {
            applied: false,
            rejection: Some(rejection_name(code)),
        },
        _ => ToggleOutcome {
            applied: false,
            rejection: Some("unexpected_response"),
        },
    })))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn tls_state(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<TlsView>>> {
    Err(AppError::ServiceUnavailable(
        "TLS control is not configured".into(),
    ))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn tls_update(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
    Json(_body): Json<TlsBody>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    Err(AppError::ServiceUnavailable(
        "TLS control is not configured".into(),
    ))
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn hid_only_state(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<HidOnlyView>>> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "device settings are not configured".into(),
        ));
    };
    let response = control
        .execute_v2(
            operation_id(&session.token),
            PlatformControlRequestV2::HidOnlyState {},
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("device settings are unavailable".into()))?;
    match response {
        ControlResponse::HidOnlyState { enabled } => {
            Ok(Json(ApiResponse::ok(HidOnlyView { enabled })))
        }
        _ => Err(AppError::ServiceUnavailable(
            "device settings are unavailable".into(),
        )),
    }
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn hid_only_update(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<HidOnlyBody>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "device settings are not configured".into(),
        ));
    };
    let response = control
        .execute_v2(
            operation_id(&session.token),
            PlatformControlRequestV2::HidOnly {
                enabled: body.enabled,
            },
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("device settings are unavailable".into()))?;
    Ok(Json(ApiResponse::ok(match response {
        ControlResponse::Applied => ToggleOutcome {
            applied: true,
            rejection: None,
        },
        ControlResponse::Rejected { code } => ToggleOutcome {
            applied: false,
            rejection: Some(rejection_name(code)),
        },
        _ => ToggleOutcome {
            applied: false,
            rejection: Some("unexpected_response"),
        },
    })))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn hid_only_state(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<HidOnlyView>>> {
    Err(AppError::ServiceUnavailable(
        "device settings are not configured".into(),
    ))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn hid_only_update(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
    Json(_body): Json<HidOnlyBody>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    Err(AppError::ServiceUnavailable(
        "device settings are not configured".into(),
    ))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn mouse_jiggler_state(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<MouseJigglerView>>> {
    Err(AppError::ServiceUnavailable(
        "device settings are not configured".into(),
    ))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn mouse_jiggler_update(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
    Json(_body): Json<MouseJigglerBody>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    Err(AppError::ServiceUnavailable(
        "device settings are not configured".into(),
    ))
}

#[cfg(feature = "restricted-control-surface")]
fn operation_id(session_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(MDNS_OP_DOMAIN);
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
fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(feature = "restricted-control-surface")]
async fn toggle(
    state: &AppState,
    session: &crate::auth::session::Session,
    enabled: bool,
) -> AppResult<ToggleOutcome> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "device settings are not configured".into(),
        ));
    };
    let _ = session_identity(&session.token);
    let response = control
        .execute_v2(
            operation_id(&session.token),
            PlatformControlRequestV2::Mdns { enabled },
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("device settings are unavailable".into()))?;
    Ok(match response {
        ControlResponse::Applied => ToggleOutcome {
            applied: true,
            rejection: None,
        },
        ControlResponse::Rejected { code } => ToggleOutcome {
            applied: false,
            rejection: Some(rejection_name(code)),
        },
        _ => ToggleOutcome {
            applied: false,
            rejection: Some("unexpected_response"),
        },
    })
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn mdns_state(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<MdnsView>>> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "device settings are not configured".into(),
        ));
    };
    let response = control
        .execute_v2(
            operation_id(&session.token),
            PlatformControlRequestV2::MdnsState {},
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("device settings are unavailable".into()))?;
    match response {
        ControlResponse::MdnsState { enabled } => Ok(Json(ApiResponse::ok(MdnsView { enabled }))),
        _ => Err(AppError::ServiceUnavailable(
            "device settings are unavailable".into(),
        )),
    }
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn mdns_enable(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    Ok(Json(ApiResponse::ok(toggle(&state, &session, true).await?)))
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn mdns_disable(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    Ok(Json(ApiResponse::ok(
        toggle(&state, &session, false).await?,
    )))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn mdns_state(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<MdnsView>>> {
    Err(AppError::ServiceUnavailable(
        "device settings are not configured".into(),
    ))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn mdns_enable(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    Err(AppError::ServiceUnavailable(
        "device settings are not configured".into(),
    ))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn mdns_disable(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    Err(AppError::ServiceUnavailable(
        "device settings are not configured".into(),
    ))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FirewallView {
    pub backend_available: bool,
    pub baseline_active: bool,
    pub mode: String,
    pub message: String,
    pub nft_installed: bool,
    pub nft_detail: String,
    pub iptables_installed: bool,
    pub ip6tables_installed: bool,
    pub rules_nft: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) struct FirewallBody {
    pub mode: String,
}

#[cfg(feature = "restricted-control-surface")]
fn firewall_view(response: ControlResponse) -> Option<FirewallView> {
    match response {
        ControlResponse::FirewallState {
            backend_available,
            baseline_active,
            mode,
            message,
            nft_installed,
            nft_detail,
            iptables_installed,
            ip6tables_installed,
            rules_nft,
        } => Some(FirewallView {
            backend_available,
            baseline_active,
            mode,
            message,
            nft_installed,
            nft_detail,
            iptables_installed,
            ip6tables_installed,
            rules_nft,
        }),
        _ => None,
    }
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn firewall_state(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<FirewallView>>> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "firewall control is not configured".into(),
        ));
    };
    let response = control
        .execute_v2(
            operation_id(&session.token),
            PlatformControlRequestV2::FirewallState {},
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("firewall control is unavailable".into()))?;
    firewall_view(response)
        .map(|view| Json(ApiResponse::ok(view)))
        .ok_or_else(|| AppError::ServiceUnavailable("firewall state is unavailable".into()))
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn firewall_set(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<FirewallBody>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    // Only the reviewed baseline is offered; the reference's other modes need
    // rules this arrangement has not reviewed.
    let enabled = match body.mode.as_str() {
        "baseline" => true,
        "disabled" | "off" | "none" => false,
        _ => {
            return Ok(Json(ApiResponse::ok(ToggleOutcome {
                applied: false,
                rejection: Some("unsupported_mode"),
            })));
        }
    };
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "firewall control is not configured".into(),
        ));
    };
    let response = control
        .execute_v2(
            operation_id(&session.token),
            PlatformControlRequestV2::FirewallBaseline { enabled },
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("firewall control is unavailable".into()))?;
    Ok(Json(ApiResponse::ok(match response {
        ControlResponse::Applied => ToggleOutcome {
            applied: true,
            rejection: None,
        },
        ControlResponse::Rejected { code } => ToggleOutcome {
            applied: false,
            rejection: Some(rejection_name(code)),
        },
        _ => ToggleOutcome {
            applied: false,
            rejection: Some("unexpected_response"),
        },
    })))
}

/// The reviewed baseline is synchronous, so the confirmation the reference
/// requires for its other modes is an accepted no-op here.
#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn firewall_confirm(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    Ok(Json(ApiResponse::ok(ToggleOutcome {
        applied: true,
        rejection: None,
    })))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn firewall_state(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<FirewallView>>> {
    Err(AppError::ServiceUnavailable(
        "firewall control is not configured".into(),
    ))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn firewall_set(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
    Json(_body): Json<FirewallBody>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    Err(AppError::ServiceUnavailable(
        "firewall control is not configured".into(),
    ))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn firewall_confirm(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    Err(AppError::ServiceUnavailable(
        "firewall control is not configured".into(),
    ))
}
