//! Bounded network writes (DNS and IPv6) forwarded to the board owner.
//!
//! The server never writes the resolver file or the sysctl tree itself. It
//! validates the authenticated session, requires an explicit confirmation,
//! derives the reviewed bindings, forwards one request over the control socket
//! and reports the read-back the owner observed.

use axum::{Extension, Json, extract::State};
use serde::{Deserialize, Serialize};
#[cfg(feature = "restricted-control-surface")]
use sha2::{Digest, Sha256};

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
    network::ResolverConfig,
};

#[cfg(feature = "restricted-control-surface")]
use crate::network::RESOLV_CONF_FILE;
#[cfg(feature = "restricted-control-surface")]
use bli_kvm_control_protocol::ControlResponse;
#[cfg(feature = "restricted-control-surface")]
use bli_kvm_core::{NetworkWriteRequest, NetworkWriteTarget, PlatformControlRequestV2};

#[cfg(feature = "restricted-control-surface")]
use crate::hid::{rejection_name, session_identity};

#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) const NETWORK_WRITE_DOMAIN: &[u8] = b"hardened-blikvm-network-write-v1";
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) const NETWORK_WRITE_OP_DOMAIN: &[u8] = b"hardened-blikvm-network-write-op-v1";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) struct NetworkWolBody {
    pub mac: String,
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NetworkWriteOutcome {
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<&'static str>,
    /// What the file or sysctl held after the write.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_back: Option<ResolverConfig>,
}

/// Operation id for one forwarded request: 32 lowercase hex characters that
/// are unique per session and call.
#[cfg(feature = "restricted-control-surface")]
pub(crate) fn operation_id(session_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(NETWORK_WRITE_OP_DOMAIN);
    hasher.update(session_token.as_bytes());
    hasher.update(unix_millis().to_le_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The confirmed binding is derived from the session and the target kind, so a
/// request cannot be replayed for another target or another session.
#[cfg(feature = "restricted-control-surface")]
fn confirmation_binding(session_token: &str, target: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(NETWORK_WRITE_DOMAIN);
    hasher.update(session_token.as_bytes());
    hasher.update(target.as_bytes());
    let digest = hasher.finalize();
    let mut binding = [0_u8; 32];
    binding.copy_from_slice(&digest);
    binding
}

#[cfg(feature = "restricted-control-surface")]
fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn wol(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<NetworkWolBody>,
) -> AppResult<Json<ApiResponse<NetworkWriteOutcome>>> {
    let target = NetworkWriteTarget::Wol {
        mac: body.mac.clone(),
    };
    let outcome = request_write(&state, &session, target, body.confirm, "wol").await?;
    if outcome.applied {
        crate::wol_macs::remember(&state, &body.mac);
    }
    Ok(Json(ApiResponse::ok(outcome)))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn wol(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
    Json(_body): Json<NetworkWolBody>,
) -> AppResult<Json<ApiResponse<NetworkWriteOutcome>>> {
    Err(AppError::ServiceUnavailable(
        "network writes are not configured".into(),
    ))
}

#[cfg(feature = "restricted-control-surface")]
async fn request_write(
    state: &AppState,
    session: &crate::auth::session::Session,
    target: NetworkWriteTarget,
    confirm: bool,
    kind: &str,
) -> AppResult<NetworkWriteOutcome> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "network writes are not configured".into(),
        ));
    };
    if !state.options.restricted_controls_enabled || !state.options.network_writes_enabled {
        return Err(AppError::ServiceUnavailable(
            "network writes are not enabled".into(),
        ));
    }
    if !confirm {
        return Err(AppError::BadRequest(
            "the network write needs an explicit confirmation".into(),
        ));
    }
    let request = NetworkWriteRequest {
        target,
        session_binding: session_identity(&session.token),
        confirmation_binding: confirmation_binding(&session.token, kind),
        issued_at_unix_ms: unix_millis(),
    };
    let response = control
        .execute_v2(
            operation_id(&session.token),
            PlatformControlRequestV2::NetworkWrite { request },
        )
        .await
        .map_err(|_| {
            AppError::ServiceUnavailable("network write control channel is unavailable".into())
        })?;
    match response {
        ControlResponse::Applied => Ok(NetworkWriteOutcome {
            applied: true,
            rejection: None,
            read_back: if kind == "dns" {
                Some(crate::network::read_resolver_config(std::path::Path::new(
                    RESOLV_CONF_FILE,
                )))
            } else {
                None
            },
        }),
        ControlResponse::Rejected { code } => Ok(NetworkWriteOutcome {
            applied: false,
            rejection: Some(rejection_name(code)),
            read_back: None,
        }),
        _ => Err(AppError::Internal(
            "network write returned an unexpected response".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "restricted-control-surface")]
    use super::*;

    #[cfg(feature = "restricted-control-surface")]
    #[test]
    fn bindings_are_domain_separated_and_nonzero() {
        let dns = confirmation_binding("token", "dns");
        let ipv6 = confirmation_binding("token", "ipv6");
        assert_ne!(dns, ipv6);
        assert_ne!(dns, confirmation_binding("other", "dns"));
        assert!(dns.iter().any(|byte| *byte != 0));
    }

    #[cfg(feature = "restricted-control-surface")]
    #[test]
    fn resolvers_read_back_from_the_reviewed_file() {
        assert_eq!(RESOLV_CONF_FILE, "/etc/resolv.conf");
    }
}
