//! Read-only system log compatibility surface.
//!
//! Ported from the Hardened NanoKVM reference response shapes and validation.
//! BLi-KVM keeps this read-only and command-free: the unprivileged network
//! process only tails plain log files. The privileged syslogd/klogd/`dmesg`
//! paths of the reference are intentionally not reproduced here.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use axum::Json;
use axum::extract::Query;
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
};
use axum::{Extension, extract::State};

const CONFIG_FILE: &str = "/etc/kvm/syslog.json";
const LOG_FILE: &str = "/tmp/hardened-syslog/messages";
const FALLBACK_LOG_FILES: [&str; 2] = ["/var/log/messages", "/tmp/messages"];
const BACKEND_LOG_FILE: &str = "/tmp/blikvm-server.log";
const KERNEL_LOG_FILE: &str = "/var/log/kern.log";
const MAX_LOG_VIEW_BYTES: usize = 256 * 1024;
const DEFAULT_LINES: usize = 200;
const MAX_LINES: usize = 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SystemLogConfig {
    pub remote_enabled: bool,
    pub remote_host: String,
    pub remote_port: u16,
    pub priority: u8,
    pub buffer_kb: u16,
    pub rotations: u8,
    pub small_output: bool,
    pub strip_timestamps: bool,
    pub kernel_enabled: bool,
    pub kernel_console_level: u8,
}

impl Default for SystemLogConfig {
    fn default() -> Self {
        Self {
            remote_enabled: false,
            remote_host: String::new(),
            remote_port: 514,
            priority: 8,
            buffer_kb: 200,
            rotations: 1,
            small_output: false,
            strip_timestamps: false,
            kernel_enabled: true,
            kernel_console_level: 7,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemLogConfigResponse {
    config: SystemLogConfig,
    local_log_file: &'static str,
    local_storage: &'static str,
    remote_protocol: &'static str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogQuery {
    kind: Option<String>,
    lines: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogResponse {
    kind: String,
    content: String,
    lines: usize,
    truncated: bool,
}

pub async fn get_config() -> AppResult<Json<ApiResponse<SystemLogConfigResponse>>> {
    Ok(Json(ApiResponse::ok(SystemLogConfigResponse {
        config: read_config()?,
        local_log_file: LOG_FILE,
        local_storage: "tmpfs",
        remote_protocol: "udp",
    })))
}

pub async fn get_messages(
    Query(query): Query<LogQuery>,
) -> AppResult<Json<ApiResponse<LogResponse>>> {
    let kind = query.kind.unwrap_or_else(|| "system".to_string());
    let lines = query.lines.unwrap_or(DEFAULT_LINES).clamp(1, MAX_LINES);

    let (content, truncated) = match kind.as_str() {
        "system" => read_first_available(&system_log_candidates(), lines)?,
        "backend" => read_tail(Path::new(BACKEND_LOG_FILE), lines)?,
        "kernel" => read_tail(Path::new(KERNEL_LOG_FILE), lines)?,
        _ => return Err(AppError::BadRequest("invalid log kind".into())),
    };

    Ok(Json(ApiResponse::ok(LogResponse {
        kind,
        content,
        lines,
        truncated,
    })))
}

fn system_log_candidates() -> Vec<&'static str> {
    std::iter::once(LOG_FILE)
        .chain(FALLBACK_LOG_FILES)
        .collect()
}

fn read_first_available(paths: &[&str], lines: usize) -> AppResult<(String, bool)> {
    for path in paths {
        if Path::new(path).exists() {
            return read_tail(Path::new(path), lines);
        }
    }
    Ok((String::new(), false))
}

fn read_tail(path: &Path, lines: usize) -> AppResult<(String, bool)> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((String::new(), false));
        }
        Err(error) => return Err(error.into()),
    };
    let length = file.metadata()?.len();
    let read_length = length.min(MAX_LOG_VIEW_BYTES as u64) as usize;
    let start = length.saturating_sub(read_length as u64);
    let mut buffer = vec![0_u8; read_length];
    file.seek(SeekFrom::Start(start))?;
    file.read_exact(&mut buffer)?;

    let truncated = start > 0;
    let mut content = String::from_utf8_lossy(&buffer).to_string();
    if truncated {
        if let Some(position) = content.find('\n') {
            content = content[position + 1..].to_string();
        }
    }
    Ok((tail_lines(&content, lines), truncated))
}

fn tail_lines(content: &str, lines: usize) -> String {
    let all_lines: Vec<&str> = content.lines().collect();
    let start = all_lines.len().saturating_sub(lines);
    let mut output = all_lines[start..].join("\n");
    if content.ends_with('\n') && !output.is_empty() {
        output.push('\n');
    }
    output
}

fn read_config() -> AppResult<SystemLogConfig> {
    match std::fs::read_to_string(CONFIG_FILE) {
        Ok(content) => {
            let config: SystemLogConfig = serde_json::from_str(&content)
                .map_err(|error| AppError::Config(format!("invalid syslog config: {error}")))?;
            validate_config(config)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(SystemLogConfig::default())
        }
        Err(error) => Err(error.into()),
    }
}

fn validate_config(mut config: SystemLogConfig) -> AppResult<SystemLogConfig> {
    config.remote_host = config.remote_host.trim().to_string();

    if config.remote_port == 0 {
        return Err(AppError::BadRequest("invalid syslog port".into()));
    }
    if !(1..=8).contains(&config.priority) {
        return Err(AppError::BadRequest("invalid syslog priority".into()));
    }
    if !(16..=1024).contains(&config.buffer_kb) {
        return Err(AppError::BadRequest(
            "log buffer must be between 16 and 1024 KiB".into(),
        ));
    }
    if config.rotations > 4 {
        return Err(AppError::BadRequest(
            "log rotations must be between 0 and 4".into(),
        ));
    }
    if !(1..=8).contains(&config.kernel_console_level) {
        return Err(AppError::BadRequest("invalid kernel console level".into()));
    }
    if config.remote_enabled && !valid_remote_host(&config.remote_host) {
        return Err(AppError::BadRequest(
            "invalid syslog host; use an IPv4 address or hostname".into(),
        ));
    }
    Ok(config)
}

fn valid_remote_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    if host.starts_with('-') || host.ends_with('-') || host.contains("..") {
        return false;
    }
    host.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_the_reference_shape() {
        let config = SystemLogConfig::default();
        assert!(!config.remote_enabled);
        assert_eq!(config.remote_port, 514);
        assert_eq!(config.priority, 8);
        assert_eq!(config.buffer_kb, 200);
        assert_eq!(config.rotations, 1);
        assert!(config.kernel_enabled);
        assert_eq!(config.kernel_console_level, 7);
    }

    #[test]
    fn config_parsing_is_defaulted_and_camel_case() {
        let config: SystemLogConfig = serde_json::from_str(
            r#"{"remoteEnabled":true,"remoteHost":"log.example","remotePort":514}"#,
        )
        .unwrap();
        let config = validate_config(config).unwrap();
        assert!(config.remote_enabled);
        assert_eq!(config.remote_host, "log.example");
        assert_eq!(config.buffer_kb, 200);
    }

    #[test]
    fn config_validation_rejects_out_of_range_values() {
        let config = SystemLogConfig {
            remote_port: 0,
            ..SystemLogConfig::default()
        };
        assert!(validate_config(config).is_err());
        let config = SystemLogConfig {
            priority: 0,
            ..SystemLogConfig::default()
        };
        assert!(validate_config(config).is_err());
        let config = SystemLogConfig {
            buffer_kb: 8,
            ..SystemLogConfig::default()
        };
        assert!(validate_config(config).is_err());
        let config = SystemLogConfig {
            rotations: 5,
            ..SystemLogConfig::default()
        };
        assert!(validate_config(config).is_err());
        let config = SystemLogConfig {
            kernel_console_level: 9,
            ..SystemLogConfig::default()
        };
        assert!(validate_config(config).is_err());
    }

    #[test]
    fn remote_host_is_only_required_when_enabled() {
        let config = SystemLogConfig {
            remote_enabled: true,
            ..SystemLogConfig::default()
        };
        assert!(validate_config(config).is_err());
        let config = SystemLogConfig {
            remote_enabled: true,
            remote_host: "bad host".to_string(),
            ..SystemLogConfig::default()
        };
        assert!(validate_config(config).is_err());
        let config = SystemLogConfig {
            remote_enabled: false,
            ..SystemLogConfig::default()
        };
        assert!(validate_config(config).is_ok());
    }

    #[test]
    fn tail_keeps_the_requested_suffix() {
        let content = "one\ntwo\nthree\nfour\n";
        assert_eq!(tail_lines(content, 2), "three\nfour\n");
        assert_eq!(tail_lines(content, 10), content);
        assert_eq!(tail_lines("only", 1), "only");
        assert_eq!(tail_lines("", 5), "");
    }

    #[test]
    fn tail_of_a_file_clamps_and_reports_truncation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("messages");
        let content: String = (0..500).map(|index| format!("line {index}\n")).collect();
        std::fs::write(&path, &content).unwrap();
        let (tail, truncated) = read_tail(&path, 3).unwrap();
        assert_eq!(tail, "line 497\nline 498\nline 499\n");
        assert!(!truncated);
        let (tail, truncated) = read_tail(Path::new("/nonexistent/messages"), 3).unwrap();
        assert_eq!(tail, "");
        assert!(!truncated);
    }

    #[test]
    fn missing_candidates_yield_empty_output() {
        let (content, truncated) =
            read_first_available(&["/nonexistent/a", "/nonexistent/b"], 10).unwrap();
        assert_eq!(content, "");
        assert!(!truncated);
    }
}

/// Writes one reviewed test line through the owner, which owns the journal.
#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn send_test(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "system log is not configured".into(),
        ));
    };
    let mut hasher = sha2::Sha256::new();
    use sha2::Digest;
    hasher.update(b"hardened-blikvm-system-log-op-v1");
    hasher.update(session.token.as_bytes());
    hasher.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0)
            .to_le_bytes(),
    );
    let operation_id: String = hasher
        .finalize()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let response = control
        .execute_v2(
            operation_id,
            bli_kvm_core::PlatformControlRequestV2::SystemLogTest {},
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("system log is unavailable".into()))?;
    Ok(Json(ApiResponse::ok(match response {
        bli_kvm_control_protocol::ControlResponse::Applied => ToggleOutcome {
            applied: true,
            rejection: None,
        },
        bli_kvm_control_protocol::ControlResponse::Rejected { code } => ToggleOutcome {
            applied: false,
            rejection: Some(crate::hid::rejection_name(code)),
        },
        _ => ToggleOutcome {
            applied: false,
            rejection: Some("unexpected_response"),
        },
    })))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn send_test(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    Err(AppError::ServiceUnavailable(
        "system log is not configured".into(),
    ))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToggleOutcome {
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<&'static str>,
}
