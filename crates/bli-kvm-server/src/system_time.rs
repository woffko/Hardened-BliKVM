//! Read-only system time compatibility surface.
//!
//! Ported from the Hardened NanoKVM reference response shape and value
//! validation, reduced to a read-only view: the unprivileged network process
//! never rewrites NTP configuration. Only `GET /api/system/time` exists here.

use std::collections::BTreeSet;
use std::path::Path;

use axum::Json;
use serde::Serialize;

use crate::error::{ApiResponse, AppError, AppResult};

const DEFAULT_TIMEZONE: &str = "Etc/UTC";
const DEFAULT_NTP_SERVERS: [&str; 4] = [
    "0.pool.ntp.org",
    "1.pool.ntp.org",
    "2.pool.ntp.org",
    "3.pool.ntp.org",
];
const MAX_NTP_SERVERS: usize = 6;
const MAX_TIMEZONE_LENGTH: usize = 128;
const MAX_NTP_SERVER_LENGTH: usize = 253;
const NTP_CONF_FILE: &str = "/etc/ntp.conf";
const LOCALTIME_FILE: &str = "/etc/localtime";
const ZONEINFO_DIR: &str = "/usr/share/zoneinfo";
const ROUTE_FILE: &str = "/proc/net/route";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeConfig {
    ntp_enabled: bool,
    timezone: String,
    servers: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeConfigResponse {
    config: TimeConfig,
    current_time: String,
    gateway: String,
    default_servers: Vec<String>,
    timezone_options: Vec<String>,
}

pub async fn get_config() -> AppResult<Json<ApiResponse<TimeConfigResponse>>> {
    let servers = current_ntp_servers();
    let config = TimeConfig {
        ntp_enabled: !servers.is_empty(),
        timezone: current_timezone().unwrap_or_else(|| DEFAULT_TIMEZONE.to_string()),
        servers,
    };
    Ok(Json(ApiResponse::ok(TimeConfigResponse {
        config,
        current_time: current_utc_time(),
        gateway: current_gateway().unwrap_or_default(),
        default_servers: default_servers(),
        timezone_options: timezone_options(Path::new(ZONEINFO_DIR)),
    })))
}

fn default_servers() -> Vec<String> {
    DEFAULT_NTP_SERVERS
        .iter()
        .map(|server| server.to_string())
        .collect()
}

fn current_ntp_servers() -> Vec<String> {
    std::fs::read_to_string(NTP_CONF_FILE)
        .ok()
        .and_then(|content| parse_ntp_servers(&content).ok())
        .filter(|servers| !servers.is_empty())
        .unwrap_or_else(default_servers)
}

fn parse_ntp_servers(content: &str) -> AppResult<Vec<String>> {
    let mut servers = Vec::new();
    for line in content.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        let mut fields = line.split_whitespace();
        if fields.next() != Some("server") {
            continue;
        }
        if let Some(server) = fields.next() {
            servers.push(server.to_string());
        }
    }
    normalize_servers(servers.iter().map(String::as_str))
}

fn normalize_servers<'a, I>(servers: I) -> AppResult<Vec<String>>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut normalized = Vec::new();
    let mut seen = BTreeSet::new();
    for server in servers {
        let server = server.trim();
        if server.is_empty() {
            continue;
        }
        if normalized.len() >= MAX_NTP_SERVERS {
            return Err(AppError::BadRequest(
                "no more than 6 NTP servers are allowed".into(),
            ));
        }
        if !valid_ntp_server(server) {
            return Err(AppError::BadRequest("invalid NTP server".into()));
        }
        if seen.insert(server.to_ascii_lowercase()) {
            normalized.push(server.to_string());
        }
    }
    Ok(normalized)
}

fn valid_ntp_server(server: &str) -> bool {
    !server.is_empty()
        && server.len() <= MAX_NTP_SERVER_LENGTH
        && !server.starts_with('-')
        && !server.ends_with('-')
        && server
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ':'))
}

fn current_timezone() -> Option<String> {
    let target = std::fs::read_link(LOCALTIME_FILE).ok()?;
    let zone = timezone_from_symlink(&target)?;
    normalize_timezone(&zone, Path::new(ZONEINFO_DIR)).ok()
}

fn timezone_from_symlink(target: &Path) -> Option<String> {
    target
        .to_string_lossy()
        .strip_prefix("/usr/share/zoneinfo/")
        .map(str::to_string)
}

fn normalize_timezone(value: &str, zoneinfo_root: &Path) -> AppResult<String> {
    let timezone = value.trim().trim_start_matches('/').to_string();
    if timezone.is_empty() || timezone.len() > MAX_TIMEZONE_LENGTH {
        return Err(AppError::BadRequest("invalid timezone".into()));
    }
    if timezone.starts_with("posix/") || timezone.starts_with("right/") {
        return Err(AppError::BadRequest("invalid timezone".into()));
    }
    if !timezone
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '_' | '-' | '+'))
    {
        return Err(AppError::BadRequest("invalid timezone".into()));
    }
    if !zoneinfo_root.join(&timezone).is_file() {
        return Err(AppError::BadRequest("unknown timezone".into()));
    }
    Ok(timezone)
}

fn timezone_options(zoneinfo_root: &Path) -> Vec<String> {
    let mut zones = BTreeSet::new();
    zones.insert(DEFAULT_TIMEZONE.to_string());

    for file in ["zone1970.tab", "zone.tab"] {
        if let Ok(content) = std::fs::read_to_string(zoneinfo_root.join(file)) {
            for line in content.lines() {
                if line.starts_with('#') || line.trim().is_empty() {
                    continue;
                }
                if let Some(zone) = line.split_whitespace().nth(2) {
                    zones.insert(zone.to_string());
                }
            }
        }
    }

    if zones.len() <= 1 {
        collect_zone_files(zoneinfo_root, "", &mut zones);
    }
    zones.into_iter().collect()
}

fn collect_zone_files(dir: &Path, prefix: &str, zones: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(name.as_str(), "posix" | "right") || name.starts_with('.') {
            continue;
        }
        let next = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let path = entry.path();
        if path.is_dir() {
            collect_zone_files(&path, &next, zones);
        } else if path.is_file() {
            zones.insert(next);
        }
    }
}

fn current_gateway() -> Option<String> {
    let content = std::fs::read_to_string(ROUTE_FILE).ok()?;
    default_ipv4_gateway(&content)
}

fn default_ipv4_gateway(content: &str) -> Option<String> {
    for line in content.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 3 || fields[1] != "00000000" {
            continue;
        }
        return parse_route_gateway(fields[2]);
    }
    None
}

fn parse_route_gateway(value: &str) -> Option<String> {
    let gateway = u32::from_str_radix(value, 16).ok()?;
    if gateway == 0 {
        return None;
    }
    Some(format!(
        "{}.{}.{}.{}",
        gateway & 0xff,
        (gateway >> 8) & 0xff,
        (gateway >> 16) & 0xff,
        (gateway >> 24) & 0xff
    ))
}

fn current_utc_time() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format_utc_time(seconds)
}

fn format_utc_time(seconds: u64) -> String {
    let days = seconds / 86_400;
    let remainder = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = remainder / 3_600;
    let minute = (remainder % 3_600) / 60;
    let second = remainder % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

fn civil_from_days(days: u64) -> (i64, u64, u64) {
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_epoch_seconds_as_utc() {
        assert_eq!(format_utc_time(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_utc_time(1_700_000_000), "2023-11-14 22:13:20 UTC");
        assert_eq!(format_utc_time(86_399), "1970-01-01 23:59:59 UTC");
    }

    #[test]
    fn parses_little_endian_route_gateway() {
        assert_eq!(
            parse_route_gateway("0101A8C0").as_deref(),
            Some("192.168.1.1")
        );
        assert_eq!(parse_route_gateway("00000000"), None);
        assert_eq!(parse_route_gateway("nothex"), None);
    }

    #[test]
    fn selects_the_default_ipv4_route_only() {
        let content = "Iface\tDestination\tGateway\neth0\t00000000\t0101A8C0\t0\t0\t0\t00000000\n";
        assert_eq!(
            default_ipv4_gateway(content).as_deref(),
            Some("192.168.1.1")
        );
        assert_eq!(default_ipv4_gateway("Iface\tDestination\tGateway\n"), None);
        assert_eq!(
            default_ipv4_gateway("Iface\tDestination\tGateway\neth0\t0000000A\t0202A8C0\n"),
            None
        );
    }

    #[test]
    fn parses_and_normalizes_ntp_servers() {
        let content =
            "# comment\nserver 0.pool.ntp.org\nserver 1.pool.ntp.org # trailing\nfoo bar\n";
        assert_eq!(
            parse_ntp_servers(content).unwrap(),
            vec!["0.pool.ntp.org", "1.pool.ntp.org"]
        );
        let duplicate = "server a.example\nserver A.EXAMPLE\n";
        assert_eq!(parse_ntp_servers(duplicate).unwrap(), vec!["a.example"]);
    }

    #[test]
    fn rejects_invalid_ntp_servers_and_too_many() {
        assert!(valid_ntp_server("0.pool.ntp.org"));
        assert!(valid_ntp_server("time-1.example"));
        assert!(!valid_ntp_server(""));
        assert!(!valid_ntp_server("-bad"));
        assert!(!valid_ntp_server("bad-"));
        assert!(!valid_ntp_server("bad server"));
        assert!(!valid_ntp_server("bad;rm"));
        assert!(!valid_ntp_server(&"a".repeat(254)));
        assert!(parse_ntp_servers("server bad;rm\n").is_err());
        let many: String = (0..7)
            .map(|index| format!("server s{index}.example\n"))
            .collect();
        assert!(parse_ntp_servers(&many).is_err());
    }

    #[test]
    fn validates_timezones_against_the_zoneinfo_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir_all(root.join("Europe")).unwrap();
        std::fs::write(root.join("Europe/Berlin"), b"").unwrap();
        assert_eq!(
            normalize_timezone("/Europe/Berlin", root).unwrap(),
            "Europe/Berlin"
        );
        assert!(normalize_timezone("Europe/Berlin", root).is_ok());
        assert!(normalize_timezone("posix/Europe/Berlin", root).is_err());
        assert!(normalize_timezone("../etc/passwd", root).is_err());
        assert!(normalize_timezone("Europe/Missing", root).is_err());
        assert!(normalize_timezone("", root).is_err());
    }

    #[test]
    fn resolves_timezone_from_a_localtime_symlink() {
        assert_eq!(
            timezone_from_symlink(Path::new("/usr/share/zoneinfo/Etc/UTC")).as_deref(),
            Some("Etc/UTC")
        );
        assert_eq!(timezone_from_symlink(Path::new("/etc/localtime")), None);
    }
}
