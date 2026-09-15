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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkDnsResponse {
    pub resolver: ResolverConfig,
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

pub(crate) async fn network_dns() -> AppResult<Json<ApiResponse<NetworkDnsResponse>>> {
    Ok(Json(ApiResponse::ok(NetworkDnsResponse {
        resolver: read_resolver_config(Path::new(RESOLV_CONF_FILE)),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_and_domains_are_parsed_and_bounded() {
        let content = "\
# comment
nameserver 10.0.0.1
nameserver 2001:db8::53  # inline comment
nameserver not-an-ip
nameserver 10.0.0.1
search lan example.internal
domain ignored.example
";
        let config = parse_resolv_conf(content);
        assert_eq!(config.source, RESOLV_CONF_FILE);
        assert_eq!(config.servers, vec!["10.0.0.1", "2001:db8::53"]);
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
    fn the_response_is_camel_case() {
        let response = NetworkDnsResponse {
            resolver: ResolverConfig {
                source: RESOLV_CONF_FILE.to_string(),
                servers: vec!["192.0.2.1".to_string()],
                search_domains: vec!["lan".to_string()],
            },
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["resolver"]["searchDomains"][0], "lan");
        assert_eq!(json["resolver"]["servers"][0], "192.0.2.1");
    }
}
