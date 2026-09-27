//! Read-only network view.
//!
//! Phase 1 left the network modules command/device-bound. This is the first
//! read-only slice of Phase 5: the unprivileged server reports the resolver
//! configuration it can read from a fixed file and never runs a command or
//! rewrites anything. Address enumeration stays out of scope by design, so the
//! response names its source instead of guessing at gateway or interface data.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;

use axum::Json;
use serde::Serialize;

use crate::error::{ApiResponse, AppResult};

pub(crate) const RESOLV_CONF_FILE: &str = "/etc/resolv.conf";
pub(crate) const NET_CLASS_ROOT: &str = "/sys/class/net";
pub(crate) const ROUTE_FILE: &str = "/proc/net/route";
const MAX_INTERFACES: usize = 16;
const MAX_INTERFACE_BYTES: usize = 32;
const MAX_SERVERS: usize = 8;
const MAX_SEARCH_DOMAINS: usize = 8;
const MAX_DOMAIN_BYTES: usize = 253;
const MAX_LINE_BYTES: usize = 1024;

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResolverConfig {
    pub source: String,
    pub servers: Vec<String>,
    pub search_domains: Vec<String>,
}

fn valid_ip(value: &str) -> bool {
    value
        .parse::<IpAddr>()
        .map(|address| match address {
            IpAddr::V4(ipv4) => ipv4 != Ipv4Addr::UNSPECIFIED,
            IpAddr::V6(ipv6) => ipv6 != Ipv6Addr::UNSPECIFIED,
        })
        .unwrap_or(false)
}

fn valid_domain(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=MAX_DOMAIN_BYTES).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'-' | b'_'))
        && !value.contains("..")
        && !value.ends_with('.')
}

/// Parse a resolver file into bounded, validated servers and search domains.
pub(crate) fn parse_resolv_conf(content: &str) -> ResolverConfig {
    let mut servers = Vec::new();
    let mut search_domains = Vec::new();
    for line in content.lines() {
        let line = line.split(['#', ';']).next().unwrap_or("").trim();
        if line.is_empty() || line.len() > MAX_LINE_BYTES {
            continue;
        }
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("nameserver") => {
                if let Some(value) = fields.next()
                    && valid_ip(value)
                    && servers.len() < MAX_SERVERS
                    && !servers.iter().any(|server| server == value)
                {
                    servers.push(value.to_string());
                }
            }
            Some("search") | Some("domain") => {
                for value in fields {
                    if valid_domain(value)
                        && search_domains.len() < MAX_SEARCH_DOMAINS
                        && !search_domains.iter().any(|domain| domain == value)
                    {
                        search_domains.push(value.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    ResolverConfig {
        source: RESOLV_CONF_FILE.to_string(),
        servers,
        search_domains,
    }
}

pub(crate) fn read_resolver_config(path: &Path) -> ResolverConfig {
    match std::fs::read_to_string(path) {
        Ok(content) => parse_resolv_conf(&content),
        Err(_) => ResolverConfig {
            source: RESOLV_CONF_FILE.to_string(),
            servers: Vec::new(),
            search_domains: Vec::new(),
        },
    }
}

fn interface_name_valid(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=MAX_INTERFACE_BYTES).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-'))
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InterfaceState {
    pub name: String,
    pub state: String,
    pub mtu: u32,
    pub wireless: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInterfacesResponse {
    pub interfaces: Vec<InterfaceState>,
    pub default_route: String,
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value.len() <= 64)
}

/// The interface owning the default IPv4 route, from the fixed route file.
pub(crate) fn parse_default_route(content: &str) -> String {
    for line in content.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 4 {
            continue;
        }
        let destination = fields[1];
        let mask = fields[7.min(fields.len() - 1)];
        if destination == "00000000" && mask == "00000000" && interface_name_valid(fields[0]) {
            return fields[0].to_string();
        }
    }
    String::new()
}

/// Read the interface list from the fixed sysfs tree.
pub(crate) fn read_interfaces(root: &Path, route_file: &Path) -> NetworkInterfacesResponse {
    let mut interfaces = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        let mut names: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| interface_name_valid(name))
            .collect();
        names.sort();
        for name in names {
            let path = root.join(&name);
            let state = read_trimmed(&path.join("operstate")).unwrap_or_else(|| "unknown".into());
            let mtu = read_trimmed(&path.join("mtu"))
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0);
            let wireless = path.join("wireless").exists();
            interfaces.push(InterfaceState {
                name,
                state,
                mtu,
                wireless,
            });
            if interfaces.len() >= MAX_INTERFACES {
                break;
            }
        }
    }
    let default_route = std::fs::read_to_string(route_file)
        .map(|content| parse_default_route(&content))
        .unwrap_or_default();
    NetworkInterfacesResponse {
        interfaces,
        default_route,
    }
}

pub(crate) async fn network_interfaces() -> AppResult<Json<ApiResponse<NetworkInterfacesResponse>>>
{
    Ok(Json(ApiResponse::ok(read_interfaces(
        Path::new(NET_CLASS_ROOT),
        Path::new(ROUTE_FILE),
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_and_domains_are_parsed_and_bounded() {
        let content = "\
# comment
nameserver 192.0.2.1
nameserver 2001:db8::53  # inline comment
nameserver not-an-ip
nameserver 192.0.2.1
search lan example.internal
domain ignored.example
";
        let config = parse_resolv_conf(content);
        assert_eq!(config.source, RESOLV_CONF_FILE);
        assert_eq!(config.servers, vec!["192.0.2.1", "2001:db8::53"]);
        assert_eq!(
            config.search_domains,
            vec!["lan", "example.internal", "ignored.example"]
        );
    }

    #[test]
    fn invalid_and_excessive_entries_are_dropped() {
        let mut content = String::new();
        for index in 0..20 {
            content.push_str(&format!("nameserver 192.0.2.{index}\n"));
        }
        content.push_str("nameserver 0.0.0.0\n");
        content.push_str("search ..bad domain_with_underscore ok.example\n");
        let config = parse_resolv_conf(&content);
        assert_eq!(config.servers.len(), MAX_SERVERS);
        assert!(!config.servers.contains(&"0.0.0.0".to_string()));
        assert_eq!(
            config.search_domains,
            vec!["domain_with_underscore", "ok.example"]
        );
    }

    #[test]
    fn a_missing_file_yields_an_empty_but_named_view() {
        let config = read_resolver_config(Path::new("/nonexistent/resolv.conf"));
        assert!(config.servers.is_empty());
        assert!(config.search_domains.is_empty());
        assert_eq!(config.source, RESOLV_CONF_FILE);
    }

    #[test]
    fn interfaces_are_listed_with_their_state_and_default_route() {
        let directory = tempfile::tempdir().unwrap();
        for (name, state, mtu, wireless) in [
            ("eth0", "up", "1500", false),
            ("wlan0", "down", "1500", true),
            ("bad name", "up", "1500", false),
        ] {
            let path = directory.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("operstate"), state).unwrap();
            std::fs::write(path.join("mtu"), mtu).unwrap();
            if wireless {
                std::fs::create_dir_all(path.join("wireless")).unwrap();
            }
        }
        let routes = tempfile::tempdir().unwrap();
        let route_file = routes.path().join("route");
        std::fs::write(
            &route_file,
            "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\n\
             eth0\t00000000\t0100A8C0\t0003\t0\t0\t0\t00000000\n",
        )
        .unwrap();
        let response = read_interfaces(directory.path(), &route_file);
        let names: Vec<&str> = response
            .interfaces
            .iter()
            .map(|interface| interface.name.as_str())
            .collect();
        assert_eq!(names, vec!["eth0", "wlan0"]);
        assert_eq!(response.interfaces[0].state, "up");
        assert_eq!(response.interfaces[0].mtu, 1500);
        assert!(!response.interfaces[0].wireless);
        assert!(response.interfaces[1].wireless);
        assert_eq!(response.default_route, "eth0");
    }

    #[test]
    fn a_missing_interface_tree_yields_an_empty_view() {
        let response = read_interfaces(
            Path::new("/nonexistent/net"),
            Path::new("/nonexistent/route"),
        );
        assert!(response.interfaces.is_empty());
        assert_eq!(response.default_route, "");
        assert_eq!(parse_default_route(""), "");
    }

    #[test]
    fn the_resolver_is_camel_case() {
        let resolver = ResolverConfig {
            source: RESOLV_CONF_FILE.to_string(),
            servers: vec!["192.0.2.1".to_string()],
            search_domains: vec!["lan".to_string()],
        };
        let json = serde_json::to_value(&resolver).unwrap();
        assert_eq!(json["searchDomains"][0], "lan");
        assert_eq!(json["servers"][0], "192.0.2.1");
    }
}
