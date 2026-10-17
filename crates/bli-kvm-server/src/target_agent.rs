//! The target agent's switch and state, kept by the board owner
//! (`docs/baselines/work-plan-20261014-target-agent.md`).
//!
//! The owner is the authority: it keeps the switch, changes the gadget and
//! keeps the channel; the server reads its state and forwards the
//! operator's switch. With the switch off the gadget has no agent's
//! collection and the owner refuses every call, whatever a client asks.

use axum::{Extension, Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
};

/// A switch changes the gadget: the whole device leaves the target and comes
/// back.
#[cfg(feature = "restricted-control-surface")]
const SWITCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentView {
    program: String,
    version: String,
    commit: String,
    /// The target's system, as the agent's hello said it.
    system: Option<bli_kvm_agent_protocol::System>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TargetAgentView {
    /// The owner keeps the agent's switch here.
    available: bool,
    /// The operator's switch.
    enabled: bool,
    /// The channel's device is open.
    open: bool,
    /// Why the channel's device is not open while it should be.
    problem: Option<String>,
    /// The connected agent.
    agent: Option<AgentView>,
    /// Why the agent's last hello was refused.
    refused: Option<String>,
    messages_in: u64,
    messages_out: u64,
    dropped: u64,
    /// The commit the board was built from, which its hello names to the
    /// agent: the page offers to update an agent of another
    /// (`docs/design/target-agent-v1.md`). Empty when not known.
    board_commit: String,
}

impl TargetAgentView {
    fn unavailable() -> Self {
        Self {
            available: false,
            enabled: false,
            open: false,
            problem: None,
            agent: None,
            refused: None,
            messages_in: 0,
            messages_out: 0,
            dropped: 0,
            board_commit: String::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SwitchBody {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CallBody {
    call: bli_kvm_agent_protocol::Call,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AnswerView {
    answer: bli_kvm_agent_protocol::Answer,
    round_trip_us: u64,
}

/// How long a call from the page waits for the agent.
const CALL_TIMEOUT_MS: u32 = 3_000;

/// Asks the owner about, or for, the agent.
#[cfg(feature = "restricted-control-surface")]
async fn owner(
    state: &AppState,
    session_token: &str,
    request: bli_kvm_core::PlatformControlRequestV2,
) -> AppResult<bli_kvm_control_protocol::ControlResponse> {
    use sha2::{Digest, Sha256};
    let unavailable = || AppError::ServiceUnavailable("the target agent is unavailable".into());
    if !state.options.restricted_controls_enabled {
        return Err(unavailable());
    }
    let socket = state
        .options
        .control_socket
        .as_ref()
        .ok_or_else(unavailable)?;
    let control = bli_kvm_control_client::ControlClient::with_timeout(socket, SWITCH_TIMEOUT)
        .map_err(|_| unavailable())?;
    let mut hasher = Sha256::new();
    hasher.update(b"hardened-blikvm-target-agent-v1");
    hasher.update(session_token.as_bytes());
    hasher.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|now| now.as_nanos() as u64)
            .unwrap_or(0)
            .to_le_bytes(),
    );
    let operation: String = hasher
        .finalize()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    control
        .execute_v2(operation, request)
        .await
        .map_err(|_| unavailable())
}

#[cfg(not(feature = "restricted-control-surface"))]
async fn owner(
    _state: &AppState,
    _session_token: &str,
    _request: bli_kvm_core::PlatformControlRequestV2,
) -> AppResult<bli_kvm_control_protocol::ControlResponse> {
    Err(AppError::ServiceUnavailable(
        "the target agent is unavailable".into(),
    ))
}

async fn view(state: &AppState, session_token: &str) -> TargetAgentView {
    use bli_kvm_control_protocol::ControlResponse;
    match owner(
        state,
        session_token,
        bli_kvm_core::PlatformControlRequestV2::TargetAgentState {},
    )
    .await
    {
        Ok(ControlResponse::TargetAgentState {
            enabled,
            open,
            problem,
            agent,
            refused,
            messages_in,
            messages_out,
            dropped,
            board_commit,
            ..
        }) => TargetAgentView {
            available: true,
            enabled,
            open,
            problem,
            agent: agent.map(|hello| AgentView {
                program: hello.program,
                version: hello.version,
                commit: hello.commit,
                system: hello.system,
            }),
            refused,
            messages_in,
            messages_out,
            dropped,
            board_commit,
        },
        _ => TargetAgentView::unavailable(),
    }
}

/// `GET /api/target-agent`: the switch, the channel and the agent.
pub(crate) async fn state(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> Json<ApiResponse<TargetAgentView>> {
    Json(ApiResponse::ok(view(&state, &session.token).await))
}

/// `POST /api/target-agent/call`: `{"call": "ping" | "system" | "monitors"
/// | "keyboard"}`, answered by the connected agent. A call other than a ping
/// reads from the target and is made only under the HID lease this session
/// holds now, which the owner checks again.
pub(crate) async fn call(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<CallBody>,
) -> AppResult<Json<ApiResponse<AnswerView>>> {
    use bli_kvm_control_protocol::ControlResponse;
    if !body.call.is_valid() {
        return Err(AppError::BadRequest("the call is out of its bounds".into()));
    }
    let lease = if !body.call.needs_lease() {
        None
    } else {
        let live = state
            .hid_ws_leases
            .lock()
            .await
            .live(&session.token, std::time::Instant::now());
        let Some((lease_id, bindings)) = live else {
            return Err(AppError::Conflict(
                "the agent is read under the keyboard's and mouse's lease: take control first"
                    .into(),
            ));
        };
        Some(bli_kvm_core::TargetAgentLease {
            lease_id,
            session_binding: bindings.session_binding,
            target_binding: bindings.target_binding,
        })
    };
    match owner(
        &state,
        &session.token,
        bli_kvm_core::PlatformControlRequestV2::TargetAgentCall {
            call: body.call,
            timeout_ms: CALL_TIMEOUT_MS,
            lease,
        },
    )
    .await?
    {
        ControlResponse::TargetAgentAnswer {
            answer,
            round_trip_us,
        } => Ok(Json(ApiResponse::ok(AnswerView {
            answer,
            round_trip_us,
        }))),
        ControlResponse::TargetAgentRefused { reason } => Err(AppError::Conflict(format!(
            "the agent did not do it: {reason}"
        ))),
        ControlResponse::Rejected { code } => Err(AppError::Conflict(format!(
            "the agent did not answer: {}",
            crate::hid::rejection_name(code)
        ))),
        _ => Err(AppError::ServiceUnavailable(
            "the target agent is unavailable".into(),
        )),
    }
}

/// `POST /api/target-agent`: `{"enabled": bool}`. Switching changes the USB
/// device the target sees: it leaves and comes back.
pub(crate) async fn set(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<SwitchBody>,
) -> AppResult<Json<ApiResponse<TargetAgentView>>> {
    use bli_kvm_control_protocol::ControlResponse;
    match owner(
        &state,
        &session.token,
        bli_kvm_core::PlatformControlRequestV2::TargetAgent {
            enabled: body.enabled,
        },
    )
    .await?
    {
        ControlResponse::Applied => Ok(Json(ApiResponse::ok(view(&state, &session.token).await))),
        ControlResponse::Rejected { code } => Err(AppError::Conflict(format!(
            "the switch was refused: {}",
            crate::hid::rejection_name(code)
        ))),
        _ => Err(AppError::ServiceUnavailable(
            "the target agent is unavailable".into(),
        )),
    }
}
