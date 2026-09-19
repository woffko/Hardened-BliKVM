//! Reviewed device toggles applied by the privileged board owner.
//!
//! The reference interface publishes the device name over mDNS. The board has
//! no avahi; `systemd-resolved` answers multicast queries itself, so the toggle
//! is one reviewed key in its configuration file plus a service restart, and a
//! failed restart restores the previous file.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};

use crate::video_settings::VideoRestartRunner;

pub const MDNS_CONF: &str = "/etc/systemd/resolved.conf";
pub const MDNS_UNIT: &str = "systemd-resolved.service";
const MDNS_KEY: &str = "MulticastDNS";

/// Restarts a reviewed unit. The video-settings surface defines the trait; the
/// device toggles use the same bounded `systemctl restart` runner.
pub type UnitRestartRunner = dyn VideoRestartRunner;

pub struct MdnsState {
    conf_path: PathBuf,
    unit: String,
    runner: Box<UnitRestartRunner>,
    current: bool,
    capability_enabled: bool,
}

impl MdnsState {
    pub fn open(
        conf_path: impl Into<PathBuf>,
        unit: impl Into<String>,
        runner: Box<UnitRestartRunner>,
        capability_enabled: bool,
    ) -> Self {
        let conf_path = conf_path.into();
        let current = std::fs::read_to_string(&conf_path)
            .map(|text| parse_mdns(&text))
            .unwrap_or(false);
        Self {
            conf_path,
            unit: unit.into(),
            runner,
            current,
            capability_enabled,
        }
    }

    pub fn current(&self) -> bool {
        self.current
    }

    pub fn handle(&mut self, enabled: bool) -> ControlResponse {
        if !self.capability_enabled {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        if enabled == self.current {
            return ControlResponse::Applied;
        }
        let previous = std::fs::read_to_string(&self.conf_path).unwrap_or_default();
        let next = render_mdns(&previous, enabled);
        if let Err(reason) = write_conf(&self.conf_path, &next) {
            eprintln!("device settings: mDNS configuration write failed: {reason}");
            return reject(ControlRejectionCode::BackendRejected);
        }
        if let Err(reason) = self.runner.restart(&self.unit) {
            eprintln!("device settings: mDNS unit restart failed: {reason}");
            if let Err(restore) = write_conf(&self.conf_path, &previous) {
                eprintln!("device settings: mDNS configuration restore failed: {restore}");
            }
            return reject(ControlRejectionCode::BackendRejected);
        }
        self.current = enabled;
        ControlResponse::Applied
    }
}

/// Reads the reviewed key: an explicit `MulticastDNS=` outside a comment wins,
/// and anything else means disabled.
fn parse_mdns(text: &str) -> bool {
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some(value) = line.strip_prefix(&format!("{MDNS_KEY}=")) {
            return value.trim().eq_ignore_ascii_case("yes");
        }
    }
    false
}

/// Replaces the reviewed key and leaves every other line untouched, so the
/// file stays an operator-readable configuration.
fn render_mdns(text: &str, enabled: bool) -> String {
    let value = if enabled { "yes" } else { "no" };
    let mut lines: Vec<String> = Vec::new();
    let mut replaced = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') && trimmed.starts_with(&format!("{MDNS_KEY}=")) {
            if !replaced {
                lines.push(format!("{MDNS_KEY}={value}"));
                replaced = true;
            }
            continue;
        }
        lines.push(line.to_string());
    }
    if !replaced {
        lines.push(format!("{MDNS_KEY}={value}"));
    }
    let mut rendered = lines.join("\n");
    rendered.push('\n');
    rendered
}

fn write_conf(path: &Path, text: &str) -> Result<(), String> {
    // The sandbox grants this file, not its directory, so the reviewed
    // configuration is rewritten in place; the caller keeps the previous
    // contents and restores them when the follow-up step fails.
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("the configuration file is unavailable: {error}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("the configuration file must be a regular file".into());
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|error| format!("the configuration file could not be opened: {error}"))?;
    file.write_all(text.as_bytes())
        .map_err(|error| format!("the configuration file could not be written: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("the configuration file could not be synced: {error}"))
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
        restarts: Arc<Mutex<Vec<String>>>,
        fail: bool,
    }

    impl VideoRestartRunner for RecordingRunner {
        fn restart(&mut self, unit: &str) -> Result<(), String> {
            if self.fail {
                return Err("runner failure".into());
            }
            self.restarts.lock().unwrap().push(unit.to_string());
            Ok(())
        }
    }

    fn state(path: &Path, fail: bool) -> (MdnsState, Arc<Mutex<Vec<String>>>) {
        let restarts = Arc::new(Mutex::new(Vec::new()));
        let runner = RecordingRunner {
            restarts: restarts.clone(),
            fail,
        };
        (
            MdnsState::open(path, MDNS_UNIT, Box::new(runner), true),
            restarts,
        )
    }

    #[test]
    fn parses_and_renders_only_the_reviewed_key() {
        let text = "[Resolve]\n#MulticastDNS=no\nLLMNR=no\n";
        assert!(!parse_mdns(text));
        let enabled = render_mdns(text, true);
        assert!(enabled.contains("MulticastDNS=yes"));
        assert!(enabled.contains("LLMNR=no"));
        assert!(enabled.contains("#MulticastDNS=no"));
        assert!(parse_mdns(&enabled));
        let disabled = render_mdns(&enabled, false);
        assert!(disabled.contains("MulticastDNS=no"));
        assert!(!disabled.contains("MulticastDNS=yes"));
        assert!(parse_mdns(&render_mdns("", true)));
        assert!(!parse_mdns("MulticastDNS=yes-but-not-really"));
    }

    #[test]
    fn toggles_the_file_and_restarts_the_unit() {
        let directory = tempfile::tempdir().unwrap();
        let conf = directory.path().join("resolved.conf");
        std::fs::write(&conf, "[Resolve]\nLLMNR=no\n").unwrap();
        let (mut state, restarts) = state(&conf, false);
        assert!(!state.current());

        assert_eq!(state.handle(true), ControlResponse::Applied);
        assert!(state.current());
        assert!(
            std::fs::read_to_string(&conf)
                .unwrap()
                .contains("MulticastDNS=yes")
        );
        assert_eq!(restarts.lock().unwrap().as_slice(), [MDNS_UNIT]);

        // An unchanged request is applied without a restart.
        assert_eq!(state.handle(true), ControlResponse::Applied);
        assert_eq!(restarts.lock().unwrap().len(), 1);

        assert_eq!(state.handle(false), ControlResponse::Applied);
        assert!(!state.current());
        assert!(
            std::fs::read_to_string(&conf)
                .unwrap()
                .contains("MulticastDNS=no")
        );
        assert_eq!(restarts.lock().unwrap().len(), 2);
    }

    #[test]
    fn restores_the_file_when_the_restart_fails() {
        let directory = tempfile::tempdir().unwrap();
        let conf = directory.path().join("resolved.conf");
        std::fs::write(&conf, "[Resolve]\nMulticastDNS=no\n").unwrap();
        let (mut state, restarts) = state(&conf, true);
        assert_eq!(
            state.handle(true),
            ControlResponse::Rejected {
                code: ControlRejectionCode::BackendRejected
            }
        );
        assert!(restarts.lock().unwrap().is_empty());
        assert!(!state.current());
        assert_eq!(
            std::fs::read_to_string(&conf).unwrap(),
            "[Resolve]\nMulticastDNS=no\n"
        );
    }

    #[test]
    fn a_disabled_capability_never_touches_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let conf = directory.path().join("resolved.conf");
        let runner = RecordingRunner::default();
        let mut state = MdnsState::open(&conf, MDNS_UNIT, Box::new(runner), false);
        assert_eq!(
            state.handle(true),
            ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled
            }
        );
        assert!(!conf.exists());
    }
}

/// When the operator asks for HID only, the owner refuses every media attach
/// and reports the state so the interface can show it.
pub const HID_ONLY_STATE: &str = "/var/lib/hardened-blikvm-gadget/hid-only.env";

pub struct HidOnlyState {
    state_path: PathBuf,
    enabled: bool,
    capability_enabled: bool,
}

impl HidOnlyState {
    pub fn open(state_path: impl Into<PathBuf>, capability_enabled: bool) -> Self {
        let state_path = state_path.into();
        let enabled = std::fs::read_to_string(&state_path)
            .map(|text| parse_flag(&text))
            .unwrap_or(false);
        Self {
            state_path,
            enabled,
            capability_enabled,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn handle(&mut self, enabled: bool) -> ControlResponse {
        if !self.capability_enabled {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        if enabled == self.enabled {
            return ControlResponse::Applied;
        }
        if let Err(reason) = write_flag(&self.state_path, enabled) {
            eprintln!("device settings: hid-only state write failed: {reason}");
            return reject(ControlRejectionCode::BackendRejected);
        }
        self.enabled = enabled;
        ControlResponse::Applied
    }
}

fn parse_flag(text: &str) -> bool {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("enabled="))
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("yes"))
}

fn render_flag(enabled: bool) -> String {
    format!("enabled={}\n", if enabled { "yes" } else { "no" })
}

fn write_flag(path: &Path, enabled: bool) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "the state file has no parent directory".to_string())?;
    let metadata = std::fs::symlink_metadata(parent)
        .map_err(|error| format!("the state directory is unavailable: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("the state directory must be a real directory".into());
    }
    let temporary = path.with_extension("env.new");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)
        .map_err(|error| format!("the temporary state file is unavailable: {error}"))?;
    file.write_all(render_flag(enabled).as_bytes())
        .map_err(|error| format!("the temporary state file could not be written: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("the temporary state file could not be synced: {error}"))?;
    drop(file);
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("the state file could not be replaced: {error}"))
}

#[cfg(test)]
mod hid_only_tests {
    use super::*;

    #[test]
    fn the_flag_persists_and_normalises() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("hid-only.env");
        let mut state = HidOnlyState::open(&path, true);
        assert!(!state.enabled());
        assert_eq!(state.handle(true), ControlResponse::Applied);
        assert!(state.enabled());
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("enabled=yes")
        );
        let reloaded = HidOnlyState::open(&path, true);
        assert!(reloaded.enabled());
        assert_eq!(state.handle(true), ControlResponse::Applied);
        assert_eq!(state.handle(false), ControlResponse::Applied);
        assert!(!state.enabled());
    }

    #[test]
    fn a_disabled_capability_never_writes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("hid-only.env");
        let mut state = HidOnlyState::open(&path, false);
        assert_eq!(
            state.handle(true),
            ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled
            }
        );
        assert!(!path.exists());
    }
}
