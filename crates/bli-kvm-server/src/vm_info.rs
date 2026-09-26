//! Read-only VM information compatibility surface.
//!
//! Ported from the Hardened NanoKVM `InfoRsp`/`HardwareRsp` response shapes.
//! The hardened server keeps this read-only and does not expose unique device
//! identifiers: `deviceKey` is empty by design. The unprivileged network
//! process does not enumerate links itself; the addresses come from the board
//! owner. The uptime, mDNS name, image and application values are real.

use axum::Json;
use serde::Serialize;

use crate::error::{ApiResponse, AppResult};

const PROC_UPTIME_FILE: &str = "/proc/uptime";
/// Written by the SD image builder; world-readable.
const IMAGE_VERSION_FILE: &str = "/etc/hardened-blikvm-image";
const OS_RELEASE_FILE: &str = "/etc/os-release";

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
            .map(|value| value.trim().chars().take(128).collect::<String>())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                std::fs::read_to_string(OS_RELEASE_FILE)
                    .ok()
                    .and_then(|text| system_release(&text))
            })
            .unwrap_or_default(),
        application: env!("CARGO_PKG_VERSION").to_string(),
        uptime: std::fs::read_to_string(PROC_UPTIME_FILE)
            .ok()
            .and_then(|value| parse_uptime(&value))
            .unwrap_or(0),
        device_key: String::new(),
    }
}

pub async fn vm_info(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    axum::Extension(crate::auth::middleware::CurrentSession(session)): axum::Extension<
        crate::auth::middleware::CurrentSession,
    >,
) -> AppResult<Json<ApiResponse<InfoResponse>>> {
    let hostname = crate::system_hostname()?;
    let mut response = info(&hostname);
    // The owner reads the addresses from NetworkManager; the unprivileged
    // server cannot enumerate links itself.
    if let Some((interfaces, _)) = crate::network_ipv4::interfaces(&state, &session.token).await {
        response.ips = interfaces
            .into_iter()
            .filter(|interface| !interface.address.is_empty())
            .map(|interface| IpInfo {
                name: interface.name,
                addr: interface.address,
                version: "IPv4".into(),
                kind: if interface.kind == "wifi" {
                    "Wireless".into()
                } else {
                    "Wired".into()
                },
            })
            .collect();
    }
    Ok(Json(ApiResponse::ok(response)))
}

pub async fn vm_hardware(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
) -> Json<ApiResponse<HardwareResponse>> {
    Json(ApiResponse::ok(HardwareResponse {
        version: state.capabilities.platform.board.clone(),
    }))
}

/// The system release when no image version was recorded, e.g.
/// `Armbian 26.8.3 jammy (Ubuntu 22.04.5 LTS)`.
fn system_release(os_release: &str) -> Option<String> {
    let field = |key: &str| {
        os_release
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .map(|value| value.trim().trim_matches('"').to_string())
            .filter(|value| !value.is_empty())
    };
    let pretty = field("PRETTY_NAME")?;
    let release = match (field("NAME"), field("VERSION")) {
        (Some(name), Some(version)) if !pretty.contains(&version) => {
            let short = version.split(" (").next().unwrap_or(&version).to_string();
            format!("{pretty} ({name} {short})")
        }
        _ => pretty,
    };
    Some(release.chars().take(128).collect())
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
    fn the_system_release_names_the_base_and_the_distribution() {
        let text = "PRETTY_NAME=\"Armbian 26.8.3 jammy\"\nNAME=\"Ubuntu\"\nVERSION=\"22.04.5 LTS (Jammy Jellyfish)\"\n";
        assert_eq!(
            system_release(text).as_deref(),
            Some("Armbian 26.8.3 jammy (Ubuntu 22.04.5 LTS)")
        );
        assert_eq!(
            system_release("PRETTY_NAME=\"Debian GNU/Linux 12\"\n").as_deref(),
            Some("Debian GNU/Linux 12")
        );
        assert_eq!(system_release(""), None);
    }

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
