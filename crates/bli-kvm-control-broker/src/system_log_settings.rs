//! Reviewed system-log actions owned by the privileged board owner.
//!
//! The reference can send a test line through its log service. This board has
//! no such service, so the owner writes one bounded line to the journal with
//! `logger`, which is the surface the interface's local view reads.

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};
use bli_kvm_core::{SystemLogKind, SystemLogSettings};

/// The journal's native datagram socket: writing here is what `systemd-cat`
/// does, and it does not depend on a syslog daemon being present.
pub const JOURNAL_SOCKET: &str = "/run/systemd/journal/socket";
pub const TEST_MESSAGE: &str = "hardened-blikvm system-log test";

/// The native journal payload: one datagram with the reviewed identifier.
pub fn journal_payload() -> String {
    format!("PRIORITY=6\nSYSLOG_IDENTIFIER=hardened-blikvm\nMESSAGE={TEST_MESSAGE}\n")
}

/// Runs the reviewed `logger` call. Injected so a test asserts the exact
/// command and the failure path.
pub trait SystemLogRunner: Send {
    /// Sends one bounded payload to the journal socket.
    fn send(&mut self, socket: &str, payload: &str) -> Result<(), String>;
}

pub struct SystemLogExecRunner;

impl SystemLogRunner for SystemLogExecRunner {
    fn send(&mut self, socket: &str, payload: &str) -> Result<(), String> {
        let sender = std::os::unix::net::UnixDatagram::unbound().map_err(|e| e.to_string())?;
        sender
            .send_to(payload.as_bytes(), socket)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

pub struct SystemLogState {
    runner: Box<dyn SystemLogRunner>,
    capability_enabled: bool,
}

impl SystemLogState {
    pub fn open(runner: Box<dyn SystemLogRunner>, capability_enabled: bool) -> Self {
        Self {
            runner,
            capability_enabled,
        }
    }

    /// Writes exactly one bounded test line to the journal.
    pub fn test_message(&mut self) -> ControlResponse {
        if !self.capability_enabled {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        match self.runner.send(JOURNAL_SOCKET, &journal_payload()) {
            Ok(_) => ControlResponse::Applied,
            Err(reason) => {
                eprintln!("system log: test message failed: {reason}");
                reject(ControlRejectionCode::BackendRejected)
            }
        }
    }
}

/// What the system's tools do for the System Log page: restart the
/// forwarder and read this boot's journal. Injected so the tests play them.
pub trait SystemLogCommands: Send {
    fn restart_forwarder(&mut self) -> Result<(), String>;
    /// At most `lines` of this boot's journal of `kind` (not `Saved`).
    fn journal(&mut self, kind: SystemLogKind, lines: u16) -> Result<String, String>;
}

/// `systemctl` and `journalctl` with a deadline and a bound on what they say.
pub struct SystemLogExecCommands;

const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const JOURNAL_OUTPUT_LIMIT: usize = 512 * 1024;

impl SystemLogCommands for SystemLogExecCommands {
    fn restart_forwarder(&mut self) -> Result<(), String> {
        let mut command = std::process::Command::new("/usr/bin/systemctl");
        command.args(["restart", bli_kvm_core::SYSTEM_LOG_UNIT]);
        crate::firewall_process::run(command, COMMAND_TIMEOUT, 64 * 1024).map(|_| ())
    }

    fn journal(&mut self, kind: SystemLogKind, lines: u16) -> Result<String, String> {
        let mut command = std::process::Command::new("/usr/bin/journalctl");
        command.args(journal_arguments(kind, lines));
        crate::firewall_process::run(command, COMMAND_TIMEOUT, JOURNAL_OUTPUT_LIMIT)
    }
}

/// The fixed arguments of a journal read: this boot, newest last.
pub fn journal_arguments(kind: SystemLogKind, lines: u16) -> Vec<String> {
    let mut arguments: Vec<String> = ["--boot", "--no-pager", "--quiet", "--output=short-iso"]
        .into_iter()
        .map(String::from)
        .collect();
    arguments.push(format!("--lines={lines}"));
    match kind {
        SystemLogKind::Kernel => arguments.push("--dmesg".into()),
        SystemLogKind::Backend => {
            arguments.push("--unit=hardened-blikvm-*".into());
            arguments.push("--unit=nginx.service".into());
        }
        SystemLogKind::System | SystemLogKind::Saved => {}
    }
    arguments
}

/// Where the System Log page's settings and the kept errors live.
pub struct SystemLogPaths {
    /// The forwarder's settings, which systemd reads for its unit.
    pub env: std::path::PathBuf,
    /// The forwarder's unit: without it the system has no forwarder.
    pub unit: std::path::PathBuf,
    /// The forwarder's state directory, with `errors.log` and `errors.log.1`.
    pub saved: std::path::PathBuf,
}

impl Default for SystemLogPaths {
    fn default() -> Self {
        Self {
            env: bli_kvm_core::SYSTEM_LOG_ENV_PATH.into(),
            unit: format!("/lib/systemd/system/{}", bli_kvm_core::SYSTEM_LOG_UNIT).into(),
            saved: "/var/lib/private/hardened-blikvm-syslog".into(),
        }
    }
}

/// The System Log page's settings and reads (plan of 2026-11-17, L3).
pub struct SystemLogConfig {
    paths: SystemLogPaths,
    commands: Box<dyn SystemLogCommands>,
}

impl SystemLogConfig {
    pub fn open(paths: SystemLogPaths, commands: Box<dyn SystemLogCommands>) -> Self {
        Self { paths, commands }
    }

    fn writable(&self) -> bool {
        self.paths.unit.is_file()
    }

    fn settings(&self) -> SystemLogSettings {
        std::fs::read_to_string(&self.paths.env)
            .ok()
            .and_then(|text| SystemLogSettings::parse_env(&text))
            .unwrap_or_default()
    }

    pub fn state(&self) -> ControlResponse {
        ControlResponse::SystemLogState {
            settings: self.settings(),
            writable: self.writable(),
        }
    }

    /// Writes the settings and restarts the forwarder; when the restart
    /// fails the previous file is put back and the forwarder started with it.
    pub fn set(&mut self, settings: &SystemLogSettings) -> ControlResponse {
        if !self.writable() {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        if !settings.is_valid() {
            return reject(ControlRejectionCode::InvalidState);
        }
        let previous = std::fs::read(&self.paths.env).ok();
        if let Err(error) = write_env(&self.paths.env, settings.render_env().as_bytes()) {
            eprintln!("system log: the settings could not be written: {error}");
            return reject(ControlRejectionCode::BackendRejected);
        }
        match self.commands.restart_forwarder() {
            Ok(()) => ControlResponse::Applied,
            Err(reason) => {
                eprintln!("system log: the forwarder did not restart: {reason}");
                let restored = match previous {
                    Some(bytes) => write_env(&self.paths.env, &bytes),
                    None => std::fs::remove_file(&self.paths.env),
                };
                if let Err(error) = restored {
                    eprintln!("system log: the previous settings could not be put back: {error}");
                }
                let _ = self.commands.restart_forwarder();
                reject(ControlRejectionCode::BackendRejected)
            }
        }
    }

    /// The tail of one log, bounded to one control message.
    pub fn read(&mut self, kind: SystemLogKind, lines: u16) -> ControlResponse {
        let text = match kind {
            SystemLogKind::Saved => {
                let mut text = String::new();
                for name in ["errors.log.1", "errors.log"] {
                    if let Ok(part) = std::fs::read(self.paths.saved.join(name)) {
                        text.push_str(&String::from_utf8_lossy(&part));
                    }
                }
                Ok(text)
            }
            _ => self.commands.journal(kind, lines),
        };
        match text {
            Ok(text) => {
                let (content, truncated) = bounded_tail(&text, usize::from(lines));
                ControlResponse::SystemLogText { content, truncated }
            }
            Err(reason) => {
                eprintln!("system log: the journal could not be read: {reason}");
                reject(ControlRejectionCode::BackendUnavailable)
            }
        }
    }
}

/// Writes the settings file whole, root's alone, or not at all.
fn write_env(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let next = path.with_extension("env.next");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&next)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&next, path)
}

/// The last `lines` lines of `text`, within `SYSTEM_LOG_MAX_TEXT_BYTES`, and
/// whether anything was left out.
pub fn bounded_tail(text: &str, lines: usize) -> (String, bool) {
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    let mut truncated = start > 0;
    let mut kept: Vec<&str> = Vec::new();
    let mut size = 0;
    for line in all[start..].iter().rev() {
        if size + line.len() + 1 > bli_kvm_core::SYSTEM_LOG_MAX_TEXT_BYTES {
            truncated = true;
            break;
        }
        size += line.len() + 1;
        kept.push(line);
    }
    kept.reverse();
    let mut content = kept.join("\n");
    if !content.is_empty() {
        content.push('\n');
    }
    (content, truncated)
}

fn reject(code: ControlRejectionCode) -> ControlResponse {
    ControlResponse::Rejected { code }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingRunner {
        fail: bool,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl SystemLogRunner for RecordingRunner {
        fn send(&mut self, socket: &str, payload: &str) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{socket} {payload}"));
            if self.fail {
                return Err("journal write failure".into());
            }
            Ok(())
        }
    }

    fn fixture(fail: bool, enabled: bool) -> (SystemLogState, Arc<Mutex<Vec<String>>>) {
        let runner = RecordingRunner {
            fail,
            ..Default::default()
        };
        let calls = runner.calls.clone();
        (SystemLogState::open(Box::new(runner), enabled), calls)
    }

    #[derive(Default, Clone)]
    struct PlayedCommands {
        restarts: Arc<Mutex<u32>>,
        fail_restart: Arc<Mutex<bool>>,
        reads: Arc<Mutex<Vec<(SystemLogKind, u16)>>>,
        journal: String,
    }

    impl SystemLogCommands for PlayedCommands {
        fn restart_forwarder(&mut self) -> Result<(), String> {
            *self.restarts.lock().unwrap() += 1;
            if *self.fail_restart.lock().unwrap() {
                return Err("unit failed".into());
            }
            Ok(())
        }

        fn journal(&mut self, kind: SystemLogKind, lines: u16) -> Result<String, String> {
            self.reads.lock().unwrap().push((kind, lines));
            Ok(self.journal.clone())
        }
    }

    fn played_config(
        with_unit: bool,
        commands: &PlayedCommands,
    ) -> (SystemLogConfig, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let unit = directory.path().join("hardened-blikvm-syslog.service");
        if with_unit {
            std::fs::write(&unit, "[Service]\n").unwrap();
        }
        let saved = directory.path().join("saved");
        std::fs::create_dir(&saved).unwrap();
        let paths = SystemLogPaths {
            env: directory.path().join("syslog.env"),
            unit,
            saved,
        };
        (
            SystemLogConfig::open(paths, Box::new(commands.clone())),
            directory,
        )
    }

    fn remote() -> SystemLogSettings {
        SystemLogSettings {
            remote_enabled: true,
            remote_host: "logs.example.test".into(),
            remote_port: 514,
            priority: 7,
            kernel_enabled: true,
        }
    }

    #[test]
    fn the_settings_are_written_for_the_forwarder_and_read_back() {
        let commands = PlayedCommands::default();
        let (mut config, directory) = played_config(true, &commands);
        assert_eq!(
            config.state(),
            ControlResponse::SystemLogState {
                settings: SystemLogSettings::default(),
                writable: true
            }
        );
        assert_eq!(config.set(&remote()), ControlResponse::Applied);
        assert_eq!(*commands.restarts.lock().unwrap(), 1);
        let text = std::fs::read_to_string(directory.path().join("syslog.env")).unwrap();
        assert!(text.contains("HARDENED_BLIKVM_SYSLOG_REMOTE=logs.example.test:514\n"));
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(directory.path().join("syslog.env"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(
            config.state(),
            ControlResponse::SystemLogState {
                settings: remote(),
                writable: true
            }
        );
    }

    #[test]
    fn a_failed_restart_puts_the_previous_settings_back() {
        let commands = PlayedCommands::default();
        let (mut config, directory) = played_config(true, &commands);
        assert_eq!(config.set(&remote()), ControlResponse::Applied);
        let before = std::fs::read(directory.path().join("syslog.env")).unwrap();
        *commands.fail_restart.lock().unwrap() = true;
        let changed = SystemLogSettings {
            priority: 3,
            ..remote()
        };
        assert_eq!(
            config.set(&changed),
            ControlResponse::Rejected {
                code: ControlRejectionCode::BackendRejected
            }
        );
        assert_eq!(
            std::fs::read(directory.path().join("syslog.env")).unwrap(),
            before
        );
        // A first write that fails leaves no file behind.
        let commands = PlayedCommands::default();
        *commands.fail_restart.lock().unwrap() = true;
        let (mut config, directory) = played_config(true, &commands);
        assert!(matches!(
            config.set(&remote()),
            ControlResponse::Rejected { .. }
        ));
        assert!(!directory.path().join("syslog.env").exists());
    }

    #[test]
    fn a_system_without_the_forwarder_is_read_only() {
        let commands = PlayedCommands::default();
        let (mut config, directory) = played_config(false, &commands);
        assert_eq!(
            config.state(),
            ControlResponse::SystemLogState {
                settings: SystemLogSettings::default(),
                writable: false
            }
        );
        assert_eq!(
            config.set(&remote()),
            ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled
            }
        );
        assert!(!directory.path().join("syslog.env").exists());
        assert_eq!(*commands.restarts.lock().unwrap(), 0);
    }

    #[test]
    fn reads_are_this_boots_journal_or_the_kept_errors_bounded() {
        let commands = PlayedCommands {
            journal: (0..50).map(|index| format!("line {index}\n")).collect(),
            ..PlayedCommands::default()
        };
        let (mut config, directory) = played_config(true, &commands);
        assert_eq!(
            config.read(SystemLogKind::Kernel, 3),
            ControlResponse::SystemLogText {
                content: "line 47\nline 48\nline 49\n".into(),
                truncated: true
            }
        );
        assert_eq!(
            *commands.reads.lock().unwrap(),
            vec![(SystemLogKind::Kernel, 3)]
        );
        std::fs::write(directory.path().join("saved/errors.log.1"), "old err\n").unwrap();
        std::fs::write(directory.path().join("saved/errors.log"), "new err\n").unwrap();
        assert_eq!(
            config.read(SystemLogKind::Saved, 100),
            ControlResponse::SystemLogText {
                content: "old err\nnew err\n".into(),
                truncated: false
            }
        );
        // Never more than one control message holds.
        let long = format!("{}\n", "x".repeat(1000)).repeat(100);
        let (content, truncated) = bounded_tail(&long, 1000);
        assert!(truncated && content.len() <= bli_kvm_core::SYSTEM_LOG_MAX_TEXT_BYTES);
        assert_eq!(
            journal_arguments(SystemLogKind::Backend, 200),
            [
                "--boot",
                "--no-pager",
                "--quiet",
                "--output=short-iso",
                "--lines=200",
                "--unit=hardened-blikvm-*",
                "--unit=nginx.service"
            ]
        );
        assert!(journal_arguments(SystemLogKind::Kernel, 1).contains(&"--dmesg".to_string()));
    }

    #[test]
    fn the_test_message_is_one_bounded_logger_call() {
        let (mut state, calls) = fixture(false, true);
        assert_eq!(state.test_message(), ControlResponse::Applied);
        let calls = calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].starts_with("/run/systemd/journal/socket"));
        assert!(calls[0].contains("SYSLOG_IDENTIFIER=hardened-blikvm"));
        assert!(calls[0].contains("system-log test"));
    }

    #[test]
    fn failures_and_a_disabled_capability_are_refused() {
        let (mut failing, calls) = fixture(true, true);
        assert_eq!(
            failing.test_message(),
            ControlResponse::Rejected {
                code: ControlRejectionCode::BackendRejected
            }
        );
        assert_eq!(calls.lock().unwrap().len(), 1);

        let (mut disabled, calls) = fixture(false, false);
        assert_eq!(
            disabled.test_message(),
            ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled
            }
        );
        assert!(calls.lock().unwrap().is_empty());
    }
}
