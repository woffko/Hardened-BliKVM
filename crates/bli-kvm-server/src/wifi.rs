//! Authenticated WiFi API (`docs/design/wifi-v1.md`). The board owner drives
//! NetworkManager; a password passes through to the owner once and is never
//! stored, logged or answered here.
use axum::{
    Extension, Json,
    extract::{Query, State},
};
use bli_kvm_control_protocol::ControlResponse;
use bli_kvm_core::{PlatformControlRequestV2, WifiProfileAction};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::auth::middleware::CurrentSession;
use crate::error::{ApiResponse, AppError, AppResult};

/// NetworkManager may take 30 s to associate, and a read can wait behind a
/// connection in progress.
const WIFI_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WifiView {
    pub available: bool,
    pub connected: bool,
    pub ssid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    pub state: String,
    pub radio: bool,
    pub signal: Option<u8>,
    pub security: Option<String>,
    /// The IPv4 address on the WiFi network, as `address/prefix`.
    pub address: Option<String>,
    pub gateway: Option<String>,
    pub dns: Vec<String>,
    pub ipv6: Option<String>,
    pub hw_address: Option<String>,
    pub channel: Option<u16>,
    pub rate_mbps: Option<u32>,
    /// WiFi is the main connection and Ethernet the backup.
    pub wifi_preferred: bool,
    /// Seconds until an unconfirmed main-connection change is reverted.
    pub revert_in_seconds: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WifiNetworkView {
    pub ssid: String,
    pub signal: u8,
    pub security: String,
    pub channel: u16,
    pub in_use: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct WifiNetworks {
    pub networks: Vec<WifiNetworkView>,
}

#[derive(Debug, Serialize)]
pub(crate) struct WifiProfiles {
    pub profiles: Vec<WifiProfileItem>,
}

#[derive(Debug, Serialize)]
pub(crate) struct WifiProfileItem {
    pub id: String,
    pub ssid: String,
    pub autoconnect: bool,
    pub active: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WifiOutcome {
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<&'static str>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WifiConnectBody {
    pub ssid: String,
    pub password: String,
    #[serde(default)]
    pub hidden: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScanQuery {
    #[serde(default)]
    pub rescan: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileBody {
    pub id: String,
    pub action: WifiProfileAction,
}

/// `{enabled}` for the radio and for WiFi as the main connection.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RadioBody {
    pub enabled: bool,
}

fn operation_id() -> AppResult<String> {
    use rand_core::RngCore;
    let mut operation = [0_u8; 16];
    rand_core::OsRng
        .try_fill_bytes(&mut operation)
        .map_err(|_| AppError::ServiceUnavailable("operation identity unavailable".into()))?;
    Ok(operation.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Server-side gate: WiFi exists only in a restricted-control build that the
/// operator enabled; the owner enforces its own capability as well.
async fn wifi_request(
    state: &AppState,
    request: PlatformControlRequestV2,
) -> AppResult<ControlResponse> {
    if !cfg!(feature = "restricted-control-surface") || !state.options.restricted_controls_enabled {
        return Err(AppError::ServiceUnavailable(
            "WiFi control is not configured".into(),
        ));
    }
    let control = state
        .options
        .control_socket
        .as_ref()
        .and_then(|socket| {
            bli_kvm_control_client::ControlClient::with_timeout(socket, WIFI_TIMEOUT).ok()
        })
        .ok_or_else(|| AppError::ServiceUnavailable("WiFi control is not configured".into()))?;
    control
        .execute_v2(operation_id()?, request)
        .await
        .map_err(|_| AppError::ServiceUnavailable("WiFi control is unavailable".into()))
}

fn outcome(response: ControlResponse) -> WifiOutcome {
    match response {
        ControlResponse::Applied => WifiOutcome {
            applied: true,
            rejection: None,
        },
        ControlResponse::Rejected { code } => WifiOutcome {
            applied: false,
            rejection: Some(crate::hid::rejection_name(code)),
        },
        _ => WifiOutcome {
            applied: false,
            rejection: Some("unexpected_response"),
        },
    }
}

fn unavailable(what: &str) -> AppError {
    AppError::ServiceUnavailable(format!("{what} is unavailable"))
}

pub(crate) async fn network_wifi(
    State(state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<WifiView>>> {
    match wifi_request(&state, PlatformControlRequestV2::WifiState {}).await? {
        ControlResponse::WifiState {
            available,
            up,
            interface,
            ssid,
            state: connection,
            radio,
            signal,
            security,
            address,
            gateway,
            dns,
            ipv6,
            hw_address,
            channel,
            rate_mbps,
            wifi_preferred,
            revert_in_seconds,
        } => Ok(Json(ApiResponse::ok(WifiView {
            available,
            connected: up,
            ssid: ssid.unwrap_or_default(),
            interface,
            state: connection,
            radio,
            signal,
            security,
            address,
            gateway,
            dns,
            ipv6,
            hw_address,
            channel,
            rate_mbps,
            wifi_preferred,
            revert_in_seconds,
        }))),
        _ => Err(unavailable("the WiFi state")),
    }
}

pub(crate) async fn network_wifi_scan(
    State(state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
    Query(query): Query<ScanQuery>,
) -> AppResult<Json<ApiResponse<WifiNetworks>>> {
    match wifi_request(
        &state,
        PlatformControlRequestV2::WifiScan {
            rescan: query.rescan,
        },
    )
    .await?
    {
        ControlResponse::WifiNetworks { networks } => Ok(Json(ApiResponse::ok(WifiNetworks {
            networks: networks
                .into_iter()
                .map(|network| WifiNetworkView {
                    ssid: network.ssid,
                    signal: network.signal,
                    security: network.security,
                    channel: network.channel,
                    in_use: network.in_use,
                })
                .collect(),
        }))),
        _ => Err(unavailable("the WiFi scan")),
    }
}

pub(crate) async fn network_wifi_saved(
    State(state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<WifiProfiles>>> {
    match wifi_request(&state, PlatformControlRequestV2::WifiSaved {}).await? {
        ControlResponse::WifiProfiles { profiles } => Ok(Json(ApiResponse::ok(WifiProfiles {
            profiles: profiles
                .into_iter()
                .map(|profile| WifiProfileItem {
                    id: profile.id,
                    ssid: profile.ssid,
                    autoconnect: profile.autoconnect,
                    active: profile.active,
                })
                .collect(),
        }))),
        _ => Err(unavailable("the saved WiFi networks")),
    }
}

pub(crate) async fn network_wifi_profile(
    State(state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
    Json(body): Json<ProfileBody>,
) -> AppResult<Json<ApiResponse<WifiOutcome>>> {
    if !bli_kvm_core::valid_wifi_profile_id(&body.id) {
        return Err(AppError::BadRequest("invalid WiFi profile".into()));
    }
    let response = wifi_request(
        &state,
        PlatformControlRequestV2::WifiProfile {
            id: body.id,
            action: body.action,
        },
    )
    .await?;
    Ok(Json(ApiResponse::ok(outcome(response))))
}

pub(crate) async fn network_wifi_connect(
    State(state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
    Json(body): Json<WifiConnectBody>,
) -> AppResult<Json<ApiResponse<WifiOutcome>>> {
    if !bli_kvm_core::valid_wifi_ssid(&body.ssid)
        || !bli_kvm_core::valid_wifi_password(&body.password)
    {
        return Err(AppError::BadRequest(
            "invalid WiFi network or password".into(),
        ));
    }
    let response = wifi_request(
        &state,
        PlatformControlRequestV2::WifiConnect {
            ssid: body.ssid,
            password: body.password,
            hidden: body.hidden,
        },
    )
    .await?;
    Ok(Json(ApiResponse::ok(outcome(response))))
}

pub(crate) async fn network_wifi_disconnect(
    State(state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<WifiOutcome>>> {
    let response = wifi_request(&state, PlatformControlRequestV2::WifiDisconnect {}).await?;
    Ok(Json(ApiResponse::ok(outcome(response))))
}

pub(crate) async fn network_wifi_radio(
    State(state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
    Json(body): Json<RadioBody>,
) -> AppResult<Json<ApiResponse<WifiOutcome>>> {
    let response = wifi_request(
        &state,
        PlatformControlRequestV2::WifiRadio {
            enabled: body.enabled,
        },
    )
    .await?;
    Ok(Json(ApiResponse::ok(outcome(response))))
}

/// Makes WiFi (or Ethernet) the main connection; the page confirms it from
/// the new path, otherwise the owner reverts it after a minute.
pub(crate) async fn network_wifi_preferred(
    State(state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
    Json(body): Json<RadioBody>,
) -> AppResult<Json<ApiResponse<WifiOutcome>>> {
    let response = wifi_request(
        &state,
        PlatformControlRequestV2::WifiPreferred {
            enabled: body.enabled,
        },
    )
    .await?;
    Ok(Json(ApiResponse::ok(outcome(response))))
}

pub(crate) async fn network_wifi_preferred_confirm(
    State(state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<WifiOutcome>>> {
    let response = wifi_request(&state, PlatformControlRequestV2::WifiPreferredConfirm {}).await?;
    Ok(Json(ApiResponse::ok(outcome(response))))
}

/// The reference also offers an access-point mode; this board serves the
/// interface over its own network, so the request is refused with a reason.
pub(crate) async fn network_wifi_ap(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
    Json(_body): Json<serde_json::Value>,
) -> AppResult<Json<ApiResponse<WifiOutcome>>> {
    Ok(Json(ApiResponse::ok(WifiOutcome {
        applied: false,
        rejection: Some("unsupported_mode"),
    })))
}
