//! The system log's settings, which the board owner writes for the system log
//! forwarder (docs/design/system-log-v1.md;
//! docs/baselines/work-plan-20261117-system-log.md, L3).
//!
//! The forwarder keeps the board's errors on the card and, while a remote
//! host is set, sends the messages more urgent than the priority to it. Its
//! unit reads the settings as its environment from `SYSTEM_LOG_ENV_PATH`.

use serde::{Deserialize, Serialize};

/// The settings file, which systemd reads for the forwarder's unit.
pub const SYSTEM_LOG_ENV_PATH: &str = "/etc/hardened-blikvm/syslog.env";
/// The forwarder's unit, installed by the system from update runtime dev47.
pub const SYSTEM_LOG_UNIT: &str = "hardened-blikvm-syslog.service";
/// The most lines a read asks for.
pub const SYSTEM_LOG_MAX_LINES: u16 = 1000;
/// The most text a read answers with: its tail, within one control message.
pub const SYSTEM_LOG_MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_FILE_BYTES: usize = 1024;

/// What the System Log page sets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemLogSettings {
    pub remote_enabled: bool,
    pub remote_host: String,
    pub remote_port: u16,
    /// Messages less urgent than this are not sent (busybox's `-l`: 1 to 8).
    pub priority: u8,
    /// Whether the kernel's lines are sent too.
    pub kernel_enabled: bool,
}

impl Default for SystemLogSettings {
    fn default() -> Self {
        Self {
            remote_enabled: false,
            remote_host: String::new(),
            remote_port: 514,
            priority: 8,
            kernel_enabled: true,
        }
    }
}

/// Which log a read returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemLogKind {
    /// This boot's journal.
    System,
    /// This boot's kernel lines.
    Kernel,
    /// This boot's lines of the board's own units.
    Backend,
    /// The errors kept on the card, of this boot and earlier ones.
    Saved,
}

/// A host name or an address the forwarder can send to: letters, digits,
/// dots, hyphens and underscores, or an IPv6 address in brackets.
pub fn system_log_host_is_valid(host: &str) -> bool {
    if let Some(inner) = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        return inner.parse::<std::net::Ipv6Addr>().is_ok();
    }
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with('-')
        && !host.ends_with('-')
        && !host.contains("..")
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_'))
}

impl SystemLogSettings {
    /// Whether the settings can be written: a priority of 1 to 8, a port, and
    /// a valid host when the remote copy is on (a host kept while it is off
    /// must be valid too, or empty).
    pub fn is_valid(&self) -> bool {
        (1..=8).contains(&self.priority)
            && self.remote_port != 0
            && if self.remote_enabled {
                system_log_host_is_valid(&self.remote_host)
            } else {
                self.remote_host.is_empty() || system_log_host_is_valid(&self.remote_host)
            }
    }

    /// The forwarder's environment file.
    pub fn render_env(&self) -> String {
        let remote = if self.remote_enabled {
            format!("{}:{}", self.remote_host, self.remote_port)
        } else {
            String::new()
        };
        format!(
            "# Written by the board owner from the System Log page. Do not edit.\n\
             HARDENED_BLIKVM_SYSLOG_REMOTE={remote}\n\
             HARDENED_BLIKVM_SYSLOG_HOST={}\n\
             HARDENED_BLIKVM_SYSLOG_PORT={}\n\
             HARDENED_BLIKVM_SYSLOG_PRIORITY={}\n\
             HARDENED_BLIKVM_SYSLOG_KERNEL={}\n",
            self.remote_host,
            self.remote_port,
            self.priority,
            u8::from(self.kernel_enabled)
        )
    }

    /// The settings a file says, if it is one this owner wrote and they are
    /// valid.
    pub fn parse_env(text: &str) -> Option<Self> {
        if text.len() > MAX_FILE_BYTES {
            return None;
        }
        let mut remote = None;
        let mut host = None;
        let mut port = None;
        let mut priority = None;
        let mut kernel = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once('=')?;
            let slot = match key {
                "HARDENED_BLIKVM_SYSLOG_REMOTE" => &mut remote,
                "HARDENED_BLIKVM_SYSLOG_HOST" => &mut host,
                "HARDENED_BLIKVM_SYSLOG_PORT" => &mut port,
                "HARDENED_BLIKVM_SYSLOG_PRIORITY" => &mut priority,
                "HARDENED_BLIKVM_SYSLOG_KERNEL" => &mut kernel,
                _ => return None,
            };
            if slot.replace(value.to_string()).is_some() {
                return None;
            }
        }
        let settings = Self {
            remote_enabled: !remote?.is_empty(),
            remote_host: host?,
            remote_port: port?.parse().ok()?,
            priority: priority?.parse().ok()?,
            kernel_enabled: match kernel?.as_str() {
                "1" => true,
                "0" => false,
                _ => return None,
            },
        };
        settings.is_valid().then_some(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_names_or_addresses() {
        for host in [
            "logs.example.test",
            "192.0.2.10",
            "log_host",
            "[2001:db8::1]",
        ] {
            assert!(system_log_host_is_valid(host), "{host}");
        }
        for host in ["", "-x", "x-", "a..b", "bad host", "a;b", "[not-v6]", "a:b"] {
            assert!(!system_log_host_is_valid(host), "{host}");
        }
        assert!(!system_log_host_is_valid(&"a".repeat(254)));
    }

    #[test]
    fn settings_are_checked() {
        assert!(SystemLogSettings::default().is_valid());
        let mut settings = SystemLogSettings {
            remote_enabled: true,
            ..SystemLogSettings::default()
        };
        assert!(!settings.is_valid(), "a remote copy needs a host");
        settings.remote_host = "192.0.2.10".into();
        assert!(settings.is_valid());
        for change in [
            |s: &mut SystemLogSettings| s.priority = 0,
            |s: &mut SystemLogSettings| s.priority = 9,
            |s: &mut SystemLogSettings| s.remote_port = 0,
            |s: &mut SystemLogSettings| s.remote_host = "bad host".into(),
        ] {
            let mut changed = settings.clone();
            change(&mut changed);
            assert!(!changed.is_valid(), "{changed:?}");
        }
        // A host kept while the copy is off must still be one.
        let kept = SystemLogSettings {
            remote_host: "x;rm".into(),
            ..SystemLogSettings::default()
        };
        assert!(!kept.is_valid());
    }

    #[test]
    fn the_file_round_trips_and_names_the_remote_copy_only_when_on() {
        let settings = SystemLogSettings {
            remote_enabled: true,
            remote_host: "logs.example.test".into(),
            remote_port: 5514,
            priority: 5,
            kernel_enabled: false,
        };
        let text = settings.render_env();
        assert!(text.contains("HARDENED_BLIKVM_SYSLOG_REMOTE=logs.example.test:5514\n"));
        assert!(text.contains("HARDENED_BLIKVM_SYSLOG_PRIORITY=5\n"));
        assert!(text.contains("HARDENED_BLIKVM_SYSLOG_KERNEL=0\n"));
        assert_eq!(SystemLogSettings::parse_env(&text), Some(settings.clone()));
        let off = SystemLogSettings {
            remote_enabled: false,
            ..settings
        };
        let text = off.render_env();
        assert!(text.contains("HARDENED_BLIKVM_SYSLOG_REMOTE=\n"));
        assert_eq!(SystemLogSettings::parse_env(&text), Some(off));
    }

    #[test]
    fn a_file_this_owner_did_not_write_is_not_believed() {
        let good = SystemLogSettings::default().render_env();
        for text in [
            String::new(),
            format!("{good}EXTRA=1\n"),
            good.replace("PRIORITY=8", "PRIORITY=0"),
            good.replace("KERNEL=1", "KERNEL=yes"),
            format!("{good}HARDENED_BLIKVM_SYSLOG_PORT=514\n"),
            "x".repeat(2000),
        ] {
            assert_eq!(SystemLogSettings::parse_env(&text), None, "{text:?}");
        }
    }
}
