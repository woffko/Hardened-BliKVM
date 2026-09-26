//! The reference's SSH switch, applied by the board owner.
//!
//! Switching on enables and starts the OpenSSH server, switching off stops
//! and disables it, so the choice survives a reboot; open sessions are not
//! cut. The state is read back from systemd every time.

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};

use crate::tailscale::{SYSTEMCTL, TailscaleRunner};

pub const SSH_UNIT: &str = "ssh.service";

pub struct SshSettings {
    runner: Box<dyn TailscaleRunner>,
    unit: String,
}

impl SshSettings {
    pub fn open(runner: Box<dyn TailscaleRunner>) -> Self {
        Self {
            runner,
            unit: SSH_UNIT.into(),
        }
    }

    fn query(&mut self, verb: &str, expected: &str) -> bool {
        self.runner
            .run(SYSTEMCTL, &[verb, &self.unit])
            .map(|output| output.trim() == expected)
            .unwrap_or(false)
    }

    pub fn state(&mut self) -> ControlResponse {
        ControlResponse::SshState {
            enabled: self.query("is-enabled", "enabled"),
            active: self.query("is-active", "active"),
        }
    }

    pub fn handle(&mut self, enabled: bool) -> ControlResponse {
        let verb = if enabled { "enable" } else { "disable" };
        match self.runner.run(SYSTEMCTL, &[verb, "--now", &self.unit]) {
            Ok(_) => {
                eprintln!(
                    "ssh: the OpenSSH server was switched {}",
                    if enabled { "on" } else { "off" }
                );
                ControlResponse::Applied
            }
            Err(reason) => {
                eprintln!("ssh: systemctl {verb} failed: {reason}");
                ControlResponse::Rejected {
                    code: ControlRejectionCode::BackendRejected,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Recorder {
        calls: Arc<Mutex<Vec<String>>>,
        enabled: Arc<Mutex<bool>>,
    }

    impl TailscaleRunner for Recorder {
        fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{program} {}", args.join(" ")));
            let mut enabled = self.enabled.lock().unwrap();
            match args {
                ["enable", "--now", _] => *enabled = true,
                ["disable", "--now", _] => *enabled = false,
                ["is-enabled", _] => {
                    return Ok(if *enabled { "enabled\n" } else { "disabled\n" }.into());
                }
                ["is-active", _] => {
                    return Ok(if *enabled { "active\n" } else { "inactive\n" }.into());
                }
                _ => return Err("unexpected".into()),
            }
            Ok(String::new())
        }

        fn path_exists(&mut self, _path: &str) -> bool {
            true
        }
    }

    #[test]
    fn the_switch_enables_and_disables_the_unit_and_reads_it_back() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut ssh = SshSettings::open(Box::new(Recorder {
            calls: Arc::clone(&calls),
            enabled: Arc::new(Mutex::new(true)),
        }));
        assert_eq!(
            ssh.state(),
            ControlResponse::SshState {
                enabled: true,
                active: true
            }
        );
        assert_eq!(ssh.handle(false), ControlResponse::Applied);
        assert_eq!(
            ssh.state(),
            ControlResponse::SshState {
                enabled: false,
                active: false
            }
        );
        assert!(
            calls
                .lock()
                .unwrap()
                .contains(&"/usr/bin/systemctl disable --now ssh.service".to_string())
        );
    }
}
