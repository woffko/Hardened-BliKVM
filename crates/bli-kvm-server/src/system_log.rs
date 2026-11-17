//! The System Log page (docs/design/system-log-v1.md;
//! docs/baselines/work-plan-20261117-system-log.md, L3).
//!
//! The page keeps Hardened NanoKVM's settings and views. On this board the
//! board owner writes the settings for the system's log forwarder, which
//! keeps the errors on the card and sends an optional remote copy, and reads
//! this boot's journal and the kept errors; the server holds no log and runs
//! no command. Of the reference's settings the remote host, its port, the
//! priority and the kernel's switch apply; the rest keep their defaults.

use axum::Json;
use axum::extract::Query;
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
};
use axum::{Extension, extract::State};

/// Where the forwarder keeps the errors on the card.
const KEPT_ERRORS_FILE: &str = "/var/lib/hardened-blikvm-syslog/errors.log";
/// The forwarder's unit, which the system installs from update runtime dev47;
/// without it the settings are shown and not written.
pub const FORWARDER_UNIT_PATH: &str = "/lib/systemd/system/hardened-blikvm-syslog.service";
const DEFAULT_LINES: usize = 200;
const MAX_LINES: usize = bli_kvm_core::SYSTEM_LOG_MAX_LINES as usize;

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

impl SystemLogConfig {
    fn from_settings(settings: &bli_kvm_core::SystemLogSettings) -> Self {
        Self {
            remote_enabled: settings.remote_enabled,
            remote_host: settings.remote_host.clone(),
            remote_port: settings.remote_port,
            priority: settings.priority,
            kernel_enabled: settings.kernel_enabled,
            ..Self::default()
        }
    }

    fn settings(&self) -> bli_kvm_core::SystemLogSettings {
        bli_kvm_core::SystemLogSettings {
            remote_enabled: self.remote_enabled,
            remote_host: self.remote_host.clone(),
            remote_port: self.remote_port,
            priority: self.priority,
            kernel_enabled: self.kernel_enabled,
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
    /// Whether the board writes the settings: its system has the forwarder.
    writable: bool,
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

/// An operation identity for one request of this session.
fn operation_id(session: &crate::auth::session::Session) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"hardened-blikvm-system-log-op-v1");
    hasher.update(session.token.as_bytes());
    hasher.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0)
            .to_le_bytes(),
    );
    hasher
        .finalize()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn ask(
    state: &AppState,
    session: &crate::auth::session::Session,
    request: bli_kvm_core::PlatformControlRequestV2,
) -> AppResult<bli_kvm_control_protocol::ControlResponse> {
    let control = state
        .control
        .as_ref()
        .ok_or_else(|| AppError::ServiceUnavailable("system log is not configured".into()))?;
    control
        .execute_v2(operation_id(session), request)
        .await
        .map_err(|_| AppError::ServiceUnavailable("system log is unavailable".into()))
}

async fn read_config(
    state: &AppState,
    session: &crate::auth::session::Session,
) -> AppResult<SystemLogConfigResponse> {
    let (settings, writable) = if state.control.is_some() {
        match ask(
            state,
            session,
            bli_kvm_core::PlatformControlRequestV2::SystemLogState {},
        )
        .await?
        {
            bli_kvm_control_protocol::ControlResponse::SystemLogState { settings, writable } => {
                (settings, writable && state.options.system_log_writes)
            }
            _ => (bli_kvm_core::SystemLogSettings::default(), false),
        }
    } else {
        (bli_kvm_core::SystemLogSettings::default(), false)
    };
    Ok(SystemLogConfigResponse {
        config: SystemLogConfig::from_settings(&settings),
        local_log_file: KEPT_ERRORS_FILE,
        local_storage: "card",
        remote_protocol: "udp",
        writable,
    })
}

/// `GET /api/system-log/config`: the settings as the board owner reads them.
pub(crate) async fn get_config(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<SystemLogConfigResponse>>> {
    Ok(Json(ApiResponse::ok(read_config(&state, &session).await?)))
}

/// `POST /api/system-log/config`: the board owner writes the settings for
/// the forwarder; the answer is the settings read back.
pub(crate) async fn set_config(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(config): Json<SystemLogConfig>,
) -> AppResult<Json<ApiResponse<SystemLogConfigResponse>>> {
    if !state.options.system_log_writes {
        return Err(AppError::BadRequest(
            "this board's system has no system log forwarder".into(),
        ));
    }
    let config = validate_config(config)?;
    match ask(
        &state,
        &session,
        bli_kvm_core::PlatformControlRequestV2::SystemLog {
            settings: config.settings(),
        },
    )
    .await?
    {
        bli_kvm_control_protocol::ControlResponse::Applied => {}
        bli_kvm_control_protocol::ControlResponse::Rejected { code } => {
            return Err(AppError::BadRequest(format!(
                "the board refused the settings: {}",
                crate::hid::rejection_name(code)
            )));
        }
        _ => {
            return Err(AppError::ServiceUnavailable(
                "system log is unavailable".into(),
            ));
        }
    }
    Ok(Json(ApiResponse::ok(read_config(&state, &session).await?)))
}

/// `GET /api/system-log/messages`: this boot's journal (`system`), its
/// kernel lines (`kernel`), the board's own units (`backend`) or the errors
/// kept on the card (`saved`), through the board owner.
pub(crate) async fn get_messages(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Query(query): Query<LogQuery>,
) -> AppResult<Json<ApiResponse<LogResponse>>> {
    let kind = query.kind.unwrap_or_else(|| "system".to_string());
    let lines = query.lines.unwrap_or(DEFAULT_LINES).clamp(1, MAX_LINES);
    let log = match kind.as_str() {
        "system" => bli_kvm_core::SystemLogKind::System,
        "kernel" => bli_kvm_core::SystemLogKind::Kernel,
        "backend" => bli_kvm_core::SystemLogKind::Backend,
        "saved" => bli_kvm_core::SystemLogKind::Saved,
        _ => return Err(AppError::BadRequest("invalid log kind".into())),
    };
    let (content, truncated) = if state.control.is_some() {
        match ask(
            &state,
            &session,
            bli_kvm_core::PlatformControlRequestV2::SystemLogRead {
                kind: log,
                lines: lines as u16,
            },
        )
        .await?
        {
            bli_kvm_control_protocol::ControlResponse::SystemLogText { content, truncated } => {
                (content, truncated)
            }
            _ => {
                return Err(AppError::ServiceUnavailable(
                    "system log is unavailable".into(),
                ));
            }
        }
    } else {
        (String::new(), false)
    };
    Ok(Json(ApiResponse::ok(LogResponse {
        kind,
        content,
        lines,
        truncated,
    })))
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
