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
    network::{Ipv6State, ResolverConfig},
};

#[cfg(feature = "restricted-control-surface")]
use crate::network::{IPV6_CONF_ROOT, RESOLV_CONF_FILE};
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
pub(crate) struct NetworkDnsBody {
    pub servers: Vec<String>,
    /// The operator confirms the write explicitly; a missing or false value is
    /// refused before the owner is asked.
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) struct NetworkIpv6Body {
    /// Either the reviewed `{interface, disabled}` shape or the reference
    /// `{mode}` shape; exactly one of the two must carry the intent.
    #[serde(default)]
    pub interface: Option<String>,
    #[serde(default)]
    pub disabled: Option<bool>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub confirm: bool,
}

/// Maps the reference mode onto the reviewed sysctl toggle. The board only
/// distinguishes disabled from automatic, so `manual` is refused rather than
/// silently downgraded.
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) fn ipv6_intent(body: &NetworkIpv6Body) -> Result<bool, &'static str> {
    match (&body.mode, body.disabled) {
        (Some(mode), _) => match mode.as_str() {
            "disabled" => Ok(true),
            "slaac" | "dhcpv6" => Ok(false),
            _ => Err("unsupported_mode"),
        },
        (None, Some(disabled)) => Ok(disabled),
        (None, None) => Err("missing_mode"),
    }
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv6_state: Option<Ipv6State>,
}

/// Operation id for one forwarded request: 32 lowercase hex characters that
/// are unique per session and call.
#[cfg(feature = "restricted-control-surface")]
fn operation_id(session_token: &str) -> String {
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
pub(crate) async fn dns(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<NetworkDnsBody>,
) -> AppResult<Json<ApiResponse<NetworkWriteOutcome>>> {
    let target = NetworkWriteTarget::Dns {
        servers: body.servers.clone(),
    };
    let outcome = request_write(&state, &session, target, body.confirm, "dns").await?;
    Ok(Json(ApiResponse::ok(outcome)))
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn ipv6(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<NetworkIpv6Body>,
) -> AppResult<Json<ApiResponse<NetworkWriteOutcome>>> {
    let disabled = match ipv6_intent(&body) {
        Ok(disabled) => disabled,
        Err(rejection) => {
            return Ok(Json(ApiResponse::ok(NetworkWriteOutcome {
                applied: false,
                rejection: Some(rejection),
                read_back: None,
                ipv6_state: None,
            })));
        }
    };
    let interface = match body.interface.clone() {
        Some(interface) if !interface.is_empty() => interface,
        // The reference omits the interface; the reviewed default is the
        // interface that carries the default route.
        _ => {
            let route = std::fs::read_to_string(crate::network::ROUTE_FILE).unwrap_or_default();
            let resolved = crate::network::parse_default_route(&route);
            if resolved.is_empty() {
                return Err(AppError::ServiceUnavailable(
                    "the default-route interface is unknown".into(),
                ));
            }
            resolved
        }
    };
    let target = NetworkWriteTarget::Ipv6 {
        interface,
        disabled,
    };
    let outcome = request_write(&state, &session, target, body.confirm, "ipv6").await?;
    Ok(Json(ApiResponse::ok(outcome)))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn dns(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
    Json(_body): Json<NetworkDnsBody>,
) -> AppResult<Json<ApiResponse<NetworkWriteOutcome>>> {
    Err(AppError::ServiceUnavailable(
        "network writes are not configured".into(),
    ))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn ipv6(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
    Json(_body): Json<NetworkIpv6Body>,
) -> AppResult<Json<ApiResponse<NetworkWriteOutcome>>> {
    Err(AppError::ServiceUnavailable(
        "network writes are not configured".into(),
    ))
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
            ipv6_state: if kind == "ipv6" {
                Some(crate::network::read_ipv6_states(std::path::Path::new(
                    IPV6_CONF_ROOT,
                )))
            } else {
                None
            },
        }),
        ControlResponse::Rejected { code } => Ok(NetworkWriteOutcome {
            applied: false,
            rejection: Some(rejection_name(code)),
            read_back: None,
            ipv6_state: None,
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
    fn ipv6_reads_back_from_the_reviewed_root() {
        assert!(IPV6_CONF_ROOT.starts_with("/proc/sys/net/ipv6"));
        assert_eq!(RESOLV_CONF_FILE, "/etc/resolv.conf");
    }
}

#[cfg(test)]
mod ipv6_intent_tests {
    use super::*;

    fn body(mode: Option<&str>, disabled: Option<bool>) -> NetworkIpv6Body {
        NetworkIpv6Body {
            interface: None,
            disabled,
            mode: mode.map(str::to_string),
            confirm: true,
        }
    }

    #[test]
    fn maps_the_reference_mode_and_the_reviewed_toggle() {
        assert_eq!(ipv6_intent(&body(Some("disabled"), None)), Ok(true));
        assert_eq!(ipv6_intent(&body(Some("slaac"), None)), Ok(false));
        assert_eq!(ipv6_intent(&body(Some("dhcpv6"), None)), Ok(false));
        assert_eq!(ipv6_intent(&body(None, Some(true))), Ok(true));
        assert_eq!(ipv6_intent(&body(None, Some(false))), Ok(false));
    }

    #[test]
    fn an_impossible_or_missing_mode_is_refused() {
        assert_eq!(
            ipv6_intent(&body(Some("manual"), None)),
            Err("unsupported_mode")
        );
        assert_eq!(
            ipv6_intent(&body(Some("nonsense"), None)),
            Err("unsupported_mode")
        );
        assert_eq!(ipv6_intent(&body(None, None)), Err("missing_mode"));
    }
}
