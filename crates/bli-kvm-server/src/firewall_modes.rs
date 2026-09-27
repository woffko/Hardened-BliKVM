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

/// The reference firewall page's status (`GET /api/system/firewall`), served
/// from the owner's mode runtime while firewall modes are enabled.
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReferenceStatus {
    config: ReferenceConfig,
    effective_mode: FirewallMode,
    moderate_active: bool,
    restricted_active: bool,
    paranoid_active: bool,
    webrtc_blocked: bool,
    paranoid_available: bool,
    confirmation_required: bool,
    https_enabled: bool,
    https_port: u16,
    backend: ReferenceBackend,
    rules: ReferenceRules,
    message: String,
}

#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
#[derive(Serialize)]
pub(crate) struct ReferenceConfig {
    mode: FirewallMode,
}

#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
#[derive(Serialize)]
pub(crate) struct ReferenceBackend {
    preferred: String,
    iptables: ReferenceTool,
    ip6tables: ReferenceTool,
    nft: ReferenceTool,
}

#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
#[derive(Serialize)]
pub(crate) struct ReferenceTool {
    installed: bool,
    detail: String,
}

#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
#[derive(Serialize)]
pub(crate) struct ReferenceRules {
    ipv4: String,
    ipv6: String,
    nft: String,
}

/// Whether the owned table admits the local WebRTC media range.
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
fn media_admitted(nft: &str) -> bool {
    let Some(start) = nft.find("table inet hardened-blikvm {") else {
        return false;
    };
    let table = &nft[start..];
    let table = &table[..table.find("\n}").unwrap_or(table.len())];
    table.lines().any(|line| {
        line.split_once("udp dport ").is_some_and(|(_, rest)| {
            let range = rest.split_whitespace().next().unwrap_or_default();
            range.split_once('-').is_some_and(|(first, last)| {
                first.parse::<u16>().is_ok() && last.parse::<u16>().is_ok()
            }) && rest.trim_end().ends_with("accept")
        })
    })
}

#[cfg(feature = "restricted-control-surface")]
async fn rules_listing(
    state: &AppState,
    family: bli_kvm_core::FirewallRulesFamily,
) -> AppResult<String> {
    use bli_kvm_control_protocol::ControlResponse;
    // A listing that changes between chunks is fetched again once.
    for _ in 0..2 {
        let mut listing = String::new();
        let mut offset = 0_u32;
        let mut complete = false;
        for _ in 0..40 {
            match execute(
                state,
                bli_kvm_core::PlatformControlRequestV2::FirewallRules { family, offset },
            )
            .await?
            {
                ControlResponse::FirewallRules {
                    total,
                    offset: at,
                    chunk,
                } if at == offset => {
                    offset = offset.saturating_add(chunk.len() as u32);
                    listing.push_str(&chunk);
                    if offset >= total || chunk.is_empty() {
                        complete = true;
                        break;
                    }
                }
                _ => break,
            }
        }
        if complete {
            return Ok(listing);
        }
    }
    Err(AppError::ServiceUnavailable(
        "the firewall rules are unavailable".into(),
    ))
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn reference_status(state: &AppState) -> AppResult<ReferenceStatus> {
    use bli_kvm_control_protocol::ControlResponse;
    use bli_kvm_core::FirewallRulesFamily;
    let ControlResponse::FirewallModeState { status } = execute(
        state,
        bli_kvm_core::PlatformControlRequestV2::FirewallModeStatus {},
    )
    .await?
    else {
        return Err(AppError::ServiceUnavailable(
            "firewall modes are unavailable".into(),
        ));
    };
    let ControlResponse::FirewallState {
        nft_installed,
        nft_detail,
        iptables_installed,
        ip6tables_installed,
        https_port,
        ..
    } = execute(
        state,
        bli_kvm_core::PlatformControlRequestV2::FirewallState {},
    )
    .await?
    else {
        return Err(AppError::ServiceUnavailable(
            "the firewall state is unavailable".into(),
        ));
    };
    let rules = ReferenceRules {
        ipv4: rules_listing(state, FirewallRulesFamily::Ipv4).await?,
        ipv6: rules_listing(state, FirewallRulesFamily::Ipv6).await?,
        nft: rules_listing(state, FirewallRulesFamily::Nft).await?,
    };
    let mode = status.mode;
    let restrictive = matches!(mode, FirewallMode::Restricted | FirewallMode::Paranoid);
    let tool = |installed: bool, detail: &str| ReferenceTool {
        installed,
        detail: if installed {
            detail.to_string()
        } else {
            String::new()
        },
    };
    Ok(ReferenceStatus {
        config: ReferenceConfig { mode },
        effective_mode: mode,
        moderate_active: mode == FirewallMode::Moderate,
        restricted_active: mode == FirewallMode::Restricted,
        paranoid_active: mode == FirewallMode::Paranoid,
        webrtc_blocked: restrictive && !media_admitted(&rules.nft),
        paranoid_available: status.configuration_ready,
        confirmation_required: status.pending_operation_id.is_some(),
        https_enabled: https_port != 0,
        https_port,
        backend: ReferenceBackend {
            preferred: "nftables".into(),
            iptables: tool(iptables_installed, "/usr/sbin/iptables"),
            ip6tables: tool(ip6tables_installed, "/usr/sbin/ip6tables"),
            nft: tool(nft_installed, &nft_detail),
        },
        message: if status.recovery_required {
            "a firewall change is being rolled back".into()
        } else if status.pending_operation_id.is_some() {
            "confirm the change to keep it; the previous mode returns after the deadline".into()
        } else {
            String::new()
        },
        rules,
    })
}

#[cfg(feature = "restricted-control-surface")]
fn refused(response: bli_kvm_control_protocol::ControlResponse) -> AppError {
    match response {
        bli_kvm_control_protocol::ControlResponse::Rejected { code } => {
            AppError::Conflict(format!(
                "the firewall change was refused: {}",
                crate::hid::rejection_name(code)
            ))
        }
        _ => AppError::ServiceUnavailable("unexpected firewall owner answer".into()),
    }
}

/// Reference `POST /api/system/firewall`: applies the mode and answers with
/// the new status; a restrictive mode stays pending until confirmed.
#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn reference_set(
    state: &AppState,
    session: &crate::auth::session::Session,
    mode: &str,
) -> AppResult<ReferenceStatus> {
    use rand_core::RngCore;
    let mode = match mode {
        "baseline" => FirewallMode::Baseline,
        "moderate" => FirewallMode::Moderate,
        "restricted" => FirewallMode::Restricted,
        "paranoid" => FirewallMode::Paranoid,
        _ => return Err(AppError::BadRequest("unsupported firewall mode".into())),
    };
    let mut operation = [0; 16];
    rand_core::OsRng
        .try_fill_bytes(&mut operation)
        .map_err(|_| AppError::ServiceUnavailable("operation identity unavailable".into()))?;
    let request = bli_kvm_core::FirewallChangeRequest {
        operation_id: operation,
        session_binding: identity(session),
        mode,
        issued_at_unix_ms: now_ms(),
    };
    match execute(
        state,
        bli_kvm_core::PlatformControlRequestV2::FirewallModeChange { request },
    )
    .await?
    {
        bli_kvm_control_protocol::ControlResponse::Applied => reference_status(state).await,
        other => Err(refused(other)),
    }
}

/// Reference `POST /api/system/firewall/confirm`: keeps the pending mode,
/// from the session that requested it; with nothing pending it only answers
/// the status.
#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn reference_confirm(
    state: &AppState,
    session: &crate::auth::session::Session,
) -> AppResult<ReferenceStatus> {
    use bli_kvm_control_protocol::ControlResponse;
    let ControlResponse::FirewallModeState { status } = execute(
        state,
        bli_kvm_core::PlatformControlRequestV2::FirewallModeStatus {},
    )
    .await?
    else {
        return Err(AppError::ServiceUnavailable(
            "firewall modes are unavailable".into(),
        ));
    };
    if let Some(operation_id) = status.pending_operation_id {
        let request = bli_kvm_core::FirewallConfirmRequest {
            operation_id,
            session_binding: identity(session),
            issued_at_unix_ms: now_ms(),
        };
        match execute(
            state,
            bli_kvm_core::PlatformControlRequestV2::FirewallModeConfirm { request },
        )
        .await?
        {
            ControlResponse::Applied => {}
            other => return Err(refused(other)),
        }
    }
    reference_status(state).await
}

#[cfg(test)]
mod reference_tests {
    use super::media_admitted;

    #[test]
    fn the_local_media_range_is_read_from_the_owned_table_only() {
        let owned = "table ip filter {\n\tchain x {\n\t\tudp dport 1-2 accept\n\t}\n}\ntable inet hardened-blikvm {\n\tchain input {\n\t\tip saddr { 10.0.0.0/8 } udp dport 20000-20099 accept\n\t}\n}\n";
        assert!(media_admitted(owned));
        let foreign_only = "table ip filter {\n\tchain x {\n\t\tudp dport 20000-20099 accept\n\t}\n}\ntable inet hardened-blikvm {\n\tchain input {\n\t\ttcp dport 443 accept\n\t}\n}\n";
        assert!(!media_admitted(foreign_only));
        assert!(!media_admitted(""));
    }
}
