//! The reference's DNS page with IPv4 addressing (`/api/network/dns`).
//!
//! The board owner reads the interfaces from NetworkManager and applies a
//! change to the active profile; an addressing change reverts after two
//! minutes unless it is confirmed (`/api/network/dns/confirm`), which the page
//! offers from the new address.

use std::net::Ipv4Addr;
use std::path::Path;

use axum::{Extension, Json, extract::State};
use bli_kvm_control_protocol::NetworkInterfaceInfo;
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
};

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DnsInfo {
    interface: String,
    #[serde(rename = "type")]
    kind: String,
    address: String,
    subnet_mask: String,
    gateway: String,
    search_domains: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NetworkConfig {
    address: String,
    subnet_mask: String,
    gateway: String,
}

/// The reference's `GetDNSRsp`, plus the manual address and a pending revert.
#[derive(Debug, Serialize)]
pub(crate) struct DnsState {
    mode: &'static str,
    servers: Vec<String>,
    effective: Vec<String>,
    dhcp: Vec<String>,
    info: DnsInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    config: Option<NetworkConfig>,
    /// Seconds until an unconfirmed addressing change reverts; 0 for none.
    pending: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DnsBody {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    servers: Vec<String>,
    #[serde(default)]
    interface: Option<String>,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    subnet_mask: Option<String>,
    #[serde(default)]
    gateway: Option<String>,
    /// Accepted from the older confirmation-style body.
    #[serde(default)]
    #[allow(dead_code)]
    confirm: bool,
}

pub(crate) fn mask(prefix: u8) -> String {
    if prefix == 0 || prefix > 32 {
        return String::new();
    }
    Ipv4Addr::from(u32::MAX << (32 - u32::from(prefix))).to_string()
}

/// The prefix length of a contiguous subnet mask.
pub(crate) fn prefix(mask: &str) -> Option<u8> {
    let value = u32::from(mask.trim().parse::<Ipv4Addr>().ok()?);
    let ones = value.leading_ones();
    (value.count_ones() == ones && ones > 0).then_some(ones as u8)
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "wifi" => "Wi-Fi",
        _ => "Ethernet",
    }
}

/// The interface that carries the default route, else the first one.
fn primary(interfaces: &[NetworkInterfaceInfo]) -> Option<&NetworkInterfaceInfo> {
    interfaces
        .iter()
        .find(|interface| !interface.gateway.is_empty())
        .or_else(|| interfaces.first())
}

pub(crate) fn view(interfaces: &[NetworkInterfaceInfo], pending: u32) -> DnsState {
    let Some(interface) = primary(interfaces) else {
        return fallback();
    };
    let manual_address = interface.method == "manual";
    let manual = manual_address || interface.manual_dns;
    DnsState {
        mode: if manual { "manual" } else { "dhcp" },
        servers: if manual {
            interface.dns.clone()
        } else {
            Vec::new()
        },
        effective: interface.dns.clone(),
        dhcp: interface.dhcp_dns.clone(),
        info: DnsInfo {
            interface: interface.name.clone(),
            kind: kind_label(&interface.kind).into(),
            address: interface.address.clone(),
            subnet_mask: mask(interface.prefix),
            gateway: interface.gateway.clone(),
            search_domains: interface.search.clone(),
        },
        config: manual_address.then(|| NetworkConfig {
            address: interface.address.clone(),
            subnet_mask: mask(interface.prefix),
            gateway: interface.gateway.clone(),
        }),
        pending,
    }
}

/// Without the owner: the resolvers in use, nothing to change.
fn fallback() -> DnsState {
    let resolver =
        crate::network::read_resolver_config(Path::new(crate::network::RESOLV_CONF_FILE));
    DnsState {
        mode: "dhcp",
        servers: Vec::new(),
        effective: resolver.servers,
        dhcp: Vec::new(),
        info: DnsInfo {
            search_domains: resolver.search_domains,
            ..DnsInfo::default()
        },
        config: None,
        pending: 0,
    }
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn interfaces(
    state: &AppState,
    session_token: &str,
) -> Option<(Vec<NetworkInterfaceInfo>, u32)> {
    use bli_kvm_control_protocol::ControlResponse;
    let control = state.control.as_ref()?;
    match control
        .execute_v2(
            crate::network_write::operation_id(session_token),
            bli_kvm_core::PlatformControlRequestV2::NetworkInfo {},
        )
        .await
        .ok()?
    {
        ControlResponse::NetworkInfo {
            interfaces,
            revert_in_seconds,
        } => Some((interfaces, revert_in_seconds)),
        _ => None,
    }
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn interfaces(
    _state: &AppState,
    _session_token: &str,
) -> Option<(Vec<NetworkInterfaceInfo>, u32)> {
    None
}

pub(crate) async fn get_dns(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> Json<ApiResponse<DnsState>> {
    Json(ApiResponse::ok(
        match interfaces(&state, &session.token).await {
            Some((interfaces, pending)) => view(&interfaces, pending),
            None => fallback(),
        },
    ))
}

fn change(body: &DnsBody, fallback_interface: &str) -> AppResult<bli_kvm_core::Ipv4Change> {
    let invalid = || AppError::BadRequest("invalid arguments".into());
    let interface = body
        .interface
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| fallback_interface.to_string());
    let dhcp = match body.mode.as_deref().unwrap_or("manual") {
        "dhcp" => true,
        "manual" => false,
        _ => return Err(invalid()),
    };
    let change = if dhcp {
        bli_kvm_core::Ipv4Change {
            interface,
            dhcp,
            dns: Vec::new(),
            address: None,
            prefix: None,
            gateway: None,
        }
    } else {
        let address = body
            .address
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty());
        let (address, prefix, gateway) = match address {
            Some(address) => (
                Some(address.split('/').next().unwrap_or_default().to_string()),
                Some(
                    body.subnet_mask
                        .as_deref()
                        .and_then(prefix)
                        .ok_or_else(invalid)?,
                ),
                Some(body.gateway.clone().unwrap_or_default().trim().to_string()),
            ),
            None => (None, None, None),
        };
        bli_kvm_core::Ipv4Change {
            interface,
            dhcp,
            dns: body
                .servers
                .iter()
                .map(|server| server.trim().to_string())
                .filter(|server| !server.is_empty())
                .collect(),
            address,
            prefix,
            gateway,
        }
    };
    change.validate().map_err(|_| invalid())?;
    Ok(change)
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn set_dns(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<DnsBody>,
) -> AppResult<Json<ApiResponse<DnsState>>> {
    use bli_kvm_control_protocol::ControlResponse;
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
    let current = interfaces(&state, &session.token).await;
    let fallback_interface = current
        .as_ref()
        .and_then(|(interfaces, _)| primary(interfaces))
        .map(|interface| interface.name.clone())
        .unwrap_or_else(|| "eth0".into());
    let change = change(&body, &fallback_interface)?;
    let response = control
        .execute_v2(
            crate::network_write::operation_id(&session.token),
            bli_kvm_core::PlatformControlRequestV2::Ipv4Set { change },
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("network writes are unavailable".into()))?;
    match response {
        ControlResponse::NetworkInfo {
            interfaces,
            revert_in_seconds,
        } => Ok(Json(ApiResponse::ok(view(&interfaces, revert_in_seconds)))),
        ControlResponse::Rejected { code } => Err(AppError::BadRequest(format!(
            "the network change was refused: {}",
            crate::hid::rejection_name(code)
        ))),
        _ => Err(AppError::ServiceUnavailable(
            "network writes are unavailable".into(),
        )),
    }
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn confirm(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<()>>> {
    use bli_kvm_control_protocol::ControlResponse;
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "network writes are not configured".into(),
        ));
    };
    match control
        .execute_v2(
            crate::network_write::operation_id(&session.token),
            bli_kvm_core::PlatformControlRequestV2::Ipv4Confirm {},
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("network writes are unavailable".into()))?
    {
        ControlResponse::Applied => Ok(Json(ApiResponse::ok(()))),
        _ => Err(AppError::BadRequest(
            "no network change is waiting for confirmation".into(),
        )),
    }
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn set_dns(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
    Json(body): Json<DnsBody>,
) -> AppResult<Json<ApiResponse<DnsState>>> {
    let _ = change(&body, "eth0");
    Err(AppError::ServiceUnavailable(
        "network writes are not configured".into(),
    ))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn confirm(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<()>>> {
    Err(AppError::ServiceUnavailable(
        "network writes are not configured".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eth0(method: &str, manual_dns: bool) -> NetworkInterfaceInfo {
        NetworkInterfaceInfo {
            name: "eth0".into(),
            kind: "ethernet".into(),
            method: method.into(),
            address: "192.0.2.118".into(),
            prefix: 24,
            gateway: "192.0.2.5".into(),
            dns: vec!["192.0.2.5".into()],
            dhcp_dns: vec!["192.0.2.5".into()],
            manual_dns,
            search: vec!["int".into()],
            ..NetworkInterfaceInfo::default()
        }
    }

    #[test]
    fn masks_and_prefixes_convert() {
        assert_eq!(mask(24), "255.255.255.0");
        assert_eq!(mask(30), "255.255.255.252");
        assert_eq!(prefix("255.255.255.0"), Some(24));
        assert_eq!(prefix("255.0.255.0"), None);
        assert_eq!(prefix("0.0.0.0"), None);
    }

    #[test]
    fn the_view_follows_the_reference_shape() {
        let dhcp = view(&[eth0("auto", false)], 0);
        assert_eq!(dhcp.mode, "dhcp");
        assert!(dhcp.servers.is_empty() && dhcp.config.is_none());
        assert_eq!(dhcp.info.subnet_mask, "255.255.255.0");
        assert_eq!(dhcp.info.kind, "Ethernet");
        let manual = view(&[eth0("manual", true)], 90);
        assert_eq!(manual.mode, "manual");
        assert_eq!(manual.servers, ["192.0.2.5"]);
        assert_eq!(manual.config.as_ref().unwrap().gateway, "192.0.2.5");
        assert_eq!(manual.pending, 90);
        let value = serde_json::to_value(&manual).unwrap();
        assert_eq!(value["info"]["subnetMask"], "255.255.255.0");
        assert_eq!(value["info"]["type"], "Ethernet");
    }

    #[test]
    fn bodies_become_validated_changes() {
        let body: DnsBody = serde_json::from_value(serde_json::json!({
            "mode": "manual",
            "servers": ["1.1.1.1", ""],
            "interface": "eth0",
            "address": "192.0.2.200",
            "subnetMask": "255.255.255.0",
            "gateway": "192.0.2.5",
        }))
        .unwrap();
        let change = change(&body, "eth0").unwrap();
        assert_eq!(change.dns, ["1.1.1.1"]);
        assert_eq!(change.prefix, Some(24));
        let dhcp: DnsBody =
            serde_json::from_value(serde_json::json!({"mode": "dhcp", "servers": []})).unwrap();
        assert!(super::change(&dhcp, "eth0").unwrap().dhcp);
        let wrong: DnsBody = serde_json::from_value(serde_json::json!({
            "mode": "manual",
            "servers": ["1.1.1.1"],
            "address": "192.0.2.200",
            "subnetMask": "255.255.255.0",
            "gateway": "198.51.100.1",
        }))
        .unwrap();
        assert!(super::change(&wrong, "eth0").is_err());
    }
}

/// The reference's IPv6 page (`/api/network/ipv6`).
pub(crate) mod ipv6 {
    use super::*;

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub(crate) struct Address {
        interface: String,
        address: String,
        prefix: u8,
        scope: &'static str,
    }

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub(crate) struct Config {
        address: String,
        prefix: u8,
        gateway: String,
    }

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub(crate) struct State {
        interface: String,
        mode: String,
        enabled: bool,
        active: bool,
        /// NetworkManager's own DHCPv6 client.
        client_available: bool,
        client_path: &'static str,
        addresses: Vec<Address>,
        gateway: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        config: Option<Config>,
        status: &'static str,
        message: String,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    #[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
    pub(crate) struct Body {
        mode: String,
        #[serde(default)]
        interface: Option<String>,
        #[serde(default)]
        address: Option<String>,
        #[serde(default)]
        prefix: Option<u8>,
        #[serde(default)]
        gateway: Option<String>,
    }

    fn scope(address: &str) -> &'static str {
        match address.parse::<std::net::Ipv6Addr>() {
            Ok(address) if address.segments()[0] & 0xffc0 == 0xfe80 => "link",
            Ok(address) if address.segments()[0] & 0xfe00 == 0xfc00 => "private",
            _ => "global",
        }
    }

    fn split(cidr: &str) -> Option<(String, u8)> {
        let (address, prefix) = cidr.split_once('/')?;
        Some((address.to_string(), prefix.parse().ok()?))
    }

    pub(crate) fn view(interfaces: &[NetworkInterfaceInfo]) -> State {
        let Some(interface) = primary(interfaces) else {
            return State {
                interface: String::new(),
                mode: "disabled".into(),
                enabled: false,
                active: false,
                client_available: false,
                client_path: "",
                addresses: Vec::new(),
                gateway: String::new(),
                config: None,
                status: "",
                message: String::new(),
            };
        };
        let addresses: Vec<Address> = interface
            .ipv6_addresses
            .iter()
            .filter_map(|cidr| split(cidr))
            .map(|(address, prefix)| Address {
                interface: interface.name.clone(),
                scope: scope(&address),
                address,
                prefix,
            })
            .collect();
        let active = addresses.iter().any(|address| address.scope != "link");
        State {
            interface: interface.name.clone(),
            mode: interface.ipv6_mode.clone(),
            enabled: interface.ipv6_mode != "disabled",
            active,
            client_available: true,
            client_path: "NetworkManager",
            addresses,
            gateway: interface.ipv6_gateway.clone(),
            config: (interface.ipv6_mode == "manual")
                .then(|| split(&interface.ipv6_manual_address))
                .flatten()
                .map(|(address, prefix)| Config {
                    address,
                    prefix,
                    gateway: interface.ipv6_manual_gateway.clone(),
                }),
            status: "",
            message: if active {
                "IPv6 is active".into()
            } else {
                String::new()
            },
        }
    }

    pub(crate) async fn get(
        State(state): axum::extract::State<AppState>,
        Extension(CurrentSession(session)): Extension<CurrentSession>,
    ) -> Json<ApiResponse<self::State>> {
        let interfaces = interfaces(&state, &session.token)
            .await
            .map(|(interfaces, _)| interfaces)
            .unwrap_or_default();
        Json(ApiResponse::ok(view(&interfaces)))
    }

    #[cfg(feature = "restricted-control-surface")]
    pub(crate) async fn set(
        axum::extract::State(state): axum::extract::State<AppState>,
        Extension(CurrentSession(session)): Extension<CurrentSession>,
        Json(body): Json<Body>,
    ) -> AppResult<Json<ApiResponse<self::State>>> {
        use bli_kvm_control_protocol::ControlResponse;
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
        let fallback = interfaces(&state, &session.token)
            .await
            .and_then(|(interfaces, _)| primary(&interfaces).map(|i| i.name.clone()))
            .unwrap_or_else(|| "eth0".into());
        let manual = body.mode == "manual";
        let change = bli_kvm_core::Ipv6Change {
            interface: body
                .interface
                .filter(|name| !name.is_empty())
                .unwrap_or(fallback),
            mode: body.mode,
            address: body.address.filter(|_| manual),
            prefix: body.prefix.filter(|_| manual),
            gateway: body.gateway.filter(|value| manual && !value.is_empty()),
        };
        change
            .validate()
            .map_err(|_| AppError::BadRequest("invalid arguments".into()))?;
        match control
            .execute_v2(
                crate::network_write::operation_id(&session.token),
                bli_kvm_core::PlatformControlRequestV2::Ipv6Set { change },
            )
            .await
            .map_err(|_| AppError::ServiceUnavailable("network writes are unavailable".into()))?
        {
            ControlResponse::NetworkInfo { interfaces, .. } => {
                Ok(Json(ApiResponse::ok(view(&interfaces))))
            }
            ControlResponse::Rejected { code } => Err(AppError::BadRequest(format!(
                "the IPv6 change was refused: {}",
                crate::hid::rejection_name(code)
            ))),
            _ => Err(AppError::ServiceUnavailable(
                "network writes are unavailable".into(),
            )),
        }
    }

    #[cfg(not(feature = "restricted-control-surface"))]
    pub(crate) async fn set(
        axum::extract::State(_state): axum::extract::State<AppState>,
        Extension(CurrentSession(_session)): Extension<CurrentSession>,
        Json(_body): Json<Body>,
    ) -> AppResult<Json<ApiResponse<self::State>>> {
        Err(AppError::ServiceUnavailable(
            "network writes are not configured".into(),
        ))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_ipv6_view_follows_the_reference_shape() {
            let eth0 = NetworkInterfaceInfo {
                name: "eth0".into(),
                gateway: "192.0.2.1".into(),
                ipv6_mode: "manual".into(),
                ipv6_addresses: vec!["2001:db8::10/64".into(), "fe80::1/64".into()],
                ipv6_gateway: "fe80::9".into(),
                ipv6_manual_address: "2001:db8::10/64".into(),
                ipv6_manual_gateway: "2001:db8::1".into(),
                ..NetworkInterfaceInfo::default()
            };
            let value = serde_json::to_value(view(&[eth0])).unwrap();
            assert_eq!(value["mode"], "manual");
            assert_eq!(value["active"], true);
            assert_eq!(value["clientAvailable"], true);
            assert_eq!(value["addresses"][1]["scope"], "link");
            assert_eq!(value["config"]["prefix"], 64);
            assert_eq!(scope("fd00::1"), "private");
        }
    }
}
