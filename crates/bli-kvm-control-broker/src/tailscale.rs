//! Reviewed Tailscale surface for the board owner.
//!
//! The reference can install, start and control Tailscale. This board ships no
//! package and no source for one, so the surface is fail-closed by design: the
//! owner reports exactly what it can observe (installed, running, enabled and
//! the account details when the CLI answers), and every action is refused
//! until a package exists. Installing from the network is never offered here.

use std::process::Command;

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};

pub const TAILSCALE_BIN: &str = "/usr/bin/tailscale";
pub const TAILSCALED_UNIT: &str = "tailscaled.service";
pub const SYSTEMCTL: &str = "/usr/bin/systemctl";

/// The actions the interface can ask for. Installing is deliberately absent:
/// it would need a reviewed package source that this board does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailscaleAction {
    Start,
    Stop,
    Restart,
    Up,
    Down,
    Login,
    Logout,
}

impl TailscaleAction {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "start" => Some(Self::Start),
            "stop" => Some(Self::Stop),
            "restart" => Some(Self::Restart),
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "login" => Some(Self::Login),
            "logout" => Some(Self::Logout),
            _ => None,
        }
    }
}

pub trait TailscaleRunner: Send {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String>;
    fn path_exists(&mut self, path: &str) -> bool;
}

pub struct SystemTailscaleRunner;

/// How long a system command the owner runs (`systemctl`, `hostnamectl`,
/// `tailscale`) may take before it is killed (code audit of 2026-10-20,
/// AUD-07).
pub const SYSTEM_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const SYSTEM_COMMAND_OUTPUT: usize = 1024 * 1024;

impl TailscaleRunner for SystemTailscaleRunner {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
        let mut command = Command::new(program);
        command.args(args);
        crate::firewall_process::run(command, SYSTEM_COMMAND_TIMEOUT, SYSTEM_COMMAND_OUTPUT)
    }

    fn path_exists(&mut self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TailscaleStatus {
    pub installed: bool,
    pub running: bool,
    pub enabled: bool,
    pub login_name: String,
    pub device_ip: String,
    pub device_name: String,
    pub message: String,
}

pub struct TailscaleState {
    runner: Box<dyn TailscaleRunner>,
    capability_enabled: bool,
}

impl TailscaleState {
    pub fn open(runner: Box<dyn TailscaleRunner>, capability_enabled: bool) -> Self {
        Self {
            runner,
            capability_enabled,
        }
    }

    pub fn installed(&mut self) -> bool {
        self.runner.path_exists(TAILSCALE_BIN)
    }

    pub fn status(&mut self) -> TailscaleStatus {
        let installed = self.installed();
        let running = self
            .runner
            .run(SYSTEMCTL, &["is-active", TAILSCALED_UNIT])
            .map(|out| out.trim() == "active")
            .unwrap_or(false);
        let enabled = self
            .runner
            .run(SYSTEMCTL, &["is-enabled", TAILSCALED_UNIT])
            .map(|out| out.trim() == "enabled")
            .unwrap_or(false);
        let mut login_name = String::new();
        let mut device_ip = String::new();
        let mut device_name = String::new();
        if installed && running {
            if let Ok(output) = self.runner.run(TAILSCALE_BIN, &["status", "--json"]) {
                let parsed = parse_status_json(&output);
                login_name = parsed.0;
                device_ip = parsed.1;
                device_name = parsed.2;
            }
        }
        let message = if !installed {
            "the tailscale package is not installed on this board; install it from a \
             reviewed source before this surface can manage it"
                .to_string()
        } else if !self.capability_enabled {
            "tailscale management is refused by default".to_string()
        } else {
            String::new()
        };
        TailscaleStatus {
            installed,
            running,
            enabled,
            login_name,
            device_ip,
            device_name,
            message,
        }
    }

    /// Runs one reviewed action. Everything is refused while the package is
    /// absent, and installation is never performed from the network.
    pub fn act(&mut self, action: TailscaleAction) -> ControlResponse {
        if !self.capability_enabled {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        if !self.installed() {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        let result = match action {
            TailscaleAction::Start => self.runner.run(SYSTEMCTL, &["start", TAILSCALED_UNIT]),
            TailscaleAction::Stop => self.runner.run(SYSTEMCTL, &["stop", TAILSCALED_UNIT]),
            TailscaleAction::Restart => self.runner.run(SYSTEMCTL, &["restart", TAILSCALED_UNIT]),
            // The reference's login/lougout are the CLI's up/down.
            // Bounded as the administration helper's is: without a login
            // `up` would wait for one.
            TailscaleAction::Up | TailscaleAction::Login => {
                self.runner.run(TAILSCALE_BIN, &["up", "--timeout=30s"])
            }
            TailscaleAction::Down | TailscaleAction::Logout => {
                self.runner.run(TAILSCALE_BIN, &["down"])
            }
        };
        match result {
            Ok(_) => ControlResponse::Applied,
            Err(reason) => {
                eprintln!("tailscale: action failed: {reason}");
                reject(ControlRejectionCode::BackendRejected)
            }
        }
    }
}

/// Reads the three fields the interface shows from `tailscale status --json`
/// without pulling a JSON dependency into the owner: the reviewed keys are
/// found by their names and read as plain strings.
fn parse_status_json(text: &str) -> (String, String, String) {
    let field = |key: &str| -> String {
        let needle = format!("\"{key}\"");
        let Some(start) = text.find(&needle) else {
            return String::new();
        };
        let rest = &text[start + needle.len()..];
        let Some(colon) = rest.find(':') else {
            return String::new();
        };
        let rest = rest[colon + 1..].trim_start();
        let Some(quote) = rest.find('"') else {
            return String::new();
        };
        let value = &rest[quote + 1..];
        value.split('"').next().unwrap_or_default().to_string()
    };
    let login = field("LoginName");
    let ips = field("TailscaleIPs");
    let name = field("DNSName");
    let ip = ips
        .trim_start_matches('[')
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    (login, ip, name)
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
        installed: bool,
        active: bool,
        enabled: bool,
        status_json: String,
        fail: bool,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl TailscaleRunner for RecordingRunner {
        fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
            let call = format!("{program} {}", args.join(" "));
            self.calls.lock().unwrap().push(call.clone());
            if self.fail {
                return Err("action failure".into());
            }
            if call.ends_with("is-active tailscaled.service") {
                return if self.active {
                    Ok("active\n".into())
                } else {
                    Err("inactive".into())
                };
            }
            if call.ends_with("is-enabled tailscaled.service") {
                return if self.enabled {
                    Ok("enabled\n".into())
                } else {
                    Err("disabled".into())
                };
            }
            if call.contains("status --json") {
                return Ok(self.status_json.clone());
            }
            Ok(String::new())
        }

        fn path_exists(&mut self, _path: &str) -> bool {
            self.installed
        }
    }

    fn fixture(
        installed: bool,
        active: bool,
        enabled: bool,
    ) -> (TailscaleState, Arc<Mutex<Vec<String>>>) {
        let runner = RecordingRunner {
            installed,
            active,
            enabled,
            status_json: "{\"LoginName\":\"ops@example.test\",\"TailscaleIPs\":[\"100.64.0.5\"],\"DNSName\":\"blikvm.tail.ts.net\"}".into(),
            ..Default::default()
        };
        let calls = runner.calls.clone();
        (TailscaleState::open(Box::new(runner), true), calls)
    }

    #[test]
    fn without_the_package_everything_is_refused() {
        let (mut state, calls) = fixture(false, false, false);
        let status = state.status();
        assert!(!status.installed);
        assert!(!status.running);
        assert!(status.message.contains("not installed"));
        assert_eq!(
            state.act(TailscaleAction::Start),
            ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled
            }
        );
        // Only the probes ran; no action reached systemd or the CLI.
        assert!(
            calls
                .lock()
                .unwrap()
                .iter()
                .all(|call| !call.contains("start "))
        );
    }

    #[test]
    fn an_installed_package_reports_the_account_and_actions_apply() {
        let (mut state, calls) = fixture(true, true, true);
        let status = state.status();
        assert!(status.installed);
        assert!(status.running);
        assert!(status.enabled);
        assert_eq!(status.login_name, "ops@example.test");
        assert_eq!(status.device_ip, "100.64.0.5");
        assert_eq!(status.device_name, "blikvm.tail.ts.net");
        assert!(status.message.is_empty());

        assert_eq!(
            state.act(TailscaleAction::Restart),
            ControlResponse::Applied
        );
        assert_eq!(state.act(TailscaleAction::Login), ControlResponse::Applied);
        let calls = calls.lock().unwrap().clone();
        assert!(
            calls
                .iter()
                .any(|call| call.ends_with("restart tailscaled.service"))
        );
        assert!(
            calls
                .iter()
                .any(|call| call.ends_with("tailscale up --timeout=30s"))
        );
    }

    #[test]
    fn a_failed_action_and_a_disabled_capability_are_refused() {
        let runner = RecordingRunner {
            installed: true,
            active: true,
            enabled: true,
            fail: true,
            ..Default::default()
        };
        let mut failing = TailscaleState::open(Box::new(runner), true);
        assert_eq!(
            failing.act(TailscaleAction::Stop),
            ControlResponse::Rejected {
                code: ControlRejectionCode::BackendRejected
            }
        );

        let runner = RecordingRunner {
            installed: true,
            ..Default::default()
        };
        let mut disabled = TailscaleState::open(Box::new(runner), false);
        assert_eq!(
            disabled.act(TailscaleAction::Up),
            ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled
            }
        );
    }
}
