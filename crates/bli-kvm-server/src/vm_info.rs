//! Read-only VM information compatibility surface.
//!
//! Ported from the Hardened NanoKVM `InfoRsp`/`HardwareRsp` response shapes.
//! The hardened server keeps this read-only and does not expose unique device
//! identifiers or enumerate addresses from the unprivileged network process:
//! `ips` is empty and `deviceKey` is empty by design. The uptime, mDNS name,
//! image and application values are real.

use axum::Json;
use serde::Serialize;

use crate::error::{ApiResponse, AppResult};

const PROC_UPTIME_FILE: &str = "/proc/uptime";
const IMAGE_VERSION_FILE: &str = "/etc/hardened-blikvm/image";

#[derive(Debug, Serialize)]
pub struct IpInfo {
    pub name: String,
    pub addr: String,
    pub version: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Serialize)]
pub struct InfoResponse {
    pub ips: Vec<IpInfo>,
    pub mdns: String,
    pub image: String,
    pub application: String,
    pub uptime: u64,
    #[serde(rename = "deviceKey")]
    pub device_key: String,
}

#[derive(Debug, Serialize)]
pub struct HardwareResponse {
    pub version: String,
}

/// Reference-compatible read-only VM info.
///
/// `ips` and `deviceKey` stay empty: the unprivileged network process does not
/// enumerate link addresses and the hardened build does not expose a unique
/// device identifier. Operaators read addresses from the authenticated
/// inventory instead.
pub fn info(hostname: &str) -> InfoResponse {
    InfoResponse {
        ips: Vec::new(),
        mdns: mdns_name(hostname),
        image: std::fs::read_to_string(IMAGE_VERSION_FILE)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_default(),
        application: env!("CARGO_PKG_VERSION").to_string(),
        uptime: std::fs::read_to_string(PROC_UPTIME_FILE)
            .ok()
            .and_then(|value| parse_uptime(&value))
            .unwrap_or(0),
        device_key: String::new(),
    }
}

pub async fn vm_info() -> AppResult<Json<ApiResponse<InfoResponse>>> {
    let hostname = crate::system_hostname()?;
    Ok(Json(ApiResponse::ok(info(&hostname))))
}

pub async fn vm_hardware(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
) -> Json<ApiResponse<HardwareResponse>> {
    Json(ApiResponse::ok(HardwareResponse {
        version: state.capabilities.platform.board.clone(),
    }))
}

fn mdns_name(hostname: &str) -> String {
    if hostname.is_empty() {
        String::new()
    } else {
        format!("{hostname}.local")
    }
}

fn parse_uptime(content: &str) -> Option<u64> {
    content
        .split_whitespace()
        .next()
        .and_then(|seconds| seconds.split('.').next())
        .and_then(|seconds| seconds.parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mdns_name_appends_the_local_suffix() {
        assert_eq!(mdns_name("kvm-1"), "kvm-1.local");
        assert_eq!(mdns_name(""), "");
    }

    #[test]
    fn uptime_is_parsed_from_the_first_field() {
        assert_eq!(parse_uptime("12345.67 89012.34\n"), Some(12345));
        assert_eq!(parse_uptime("0.00 0.00"), Some(0));
        assert_eq!(parse_uptime(""), None);
        assert_eq!(parse_uptime("not-a-number 1.0"), None);
    }

    #[test]
    fn info_keeps_ips_and_device_key_empty() {
        let response = info("kvm-1");
        assert!(response.ips.is_empty());
        assert_eq!(response.device_key, "");
        assert_eq!(response.mdns, "kvm-1.local");
        assert_eq!(response.application, env!("CARGO_PKG_VERSION"));
    }
}
