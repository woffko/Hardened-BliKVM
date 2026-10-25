//! The QUIC transport's switch, applied by the board owner.
//!
//! Switching on enables and starts the gateway's unit, switching off stops
//! and disables it, so the choice survives a reboot and nothing listens on
//! the gateway's port while it is off. The state is read back from systemd
//! every time. See `docs/design/quic-transport-v1.md`.

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};

use crate::tailscale::{SYSTEMCTL, TailscaleRunner};

pub const QUIC_UNIT: &str = "hardened-blikvm-quic.service";

pub struct QuicSettings {
    runner: Box<dyn TailscaleRunner>,
}

impl QuicSettings {
    pub fn open(runner: Box<dyn TailscaleRunner>) -> Self {
        Self { runner }
    }

    fn query(&mut self, verb: &str, expected: &str) -> bool {
        self.runner
            .run(SYSTEMCTL, &[verb, QUIC_UNIT])
            .map(|output| output.trim() == expected)
            .unwrap_or(false)
    }

    pub fn state(&mut self) -> ControlResponse {
        ControlResponse::QuicState {
            enabled: self.query("is-enabled", "enabled"),
            active: self.query("is-active", "active"),
        }
    }

    pub fn handle(&mut self, enabled: bool) -> ControlResponse {
        let verb = if enabled { "enable" } else { "disable" };
        match self.runner.run(SYSTEMCTL, &[verb, "--now", QUIC_UNIT]) {
            Ok(_) => {
                eprintln!(
                    "quic: the gateway was switched {}",
                    if enabled { "on" } else { "off" }
                );
                ControlResponse::Applied
            }
            Err(reason) => {
                eprintln!("quic: systemctl {verb} failed: {reason}");
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
        fail: bool,
    }

    impl TailscaleRunner for Recorder {
        fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{program} {}", args.join(" ")));
            let mut enabled = self.enabled.lock().unwrap();
            match args {
                ["enable", "--now", QUIC_UNIT] | ["disable", "--now", QUIC_UNIT] if self.fail => {
                    return Err("unit not found".into());
                }
                ["enable", "--now", QUIC_UNIT] => *enabled = true,
                ["disable", "--now", QUIC_UNIT] => *enabled = false,
                ["is-enabled", QUIC_UNIT] => {
                    return Ok(if *enabled { "enabled\n" } else { "disabled\n" }.into());
                }
                ["is-active", QUIC_UNIT] => {
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
    fn the_switch_starts_off_and_enables_and_disables_the_unit() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut quic = QuicSettings::open(Box::new(Recorder {
            calls: Arc::clone(&calls),
            enabled: Arc::new(Mutex::new(false)),
            fail: false,
        }));
        assert_eq!(
            quic.state(),
            ControlResponse::QuicState {
                enabled: false,
                active: false
            }
        );
        assert_eq!(quic.handle(true), ControlResponse::Applied);
        assert_eq!(
            quic.state(),
            ControlResponse::QuicState {
                enabled: true,
                active: true
            }
        );
        assert_eq!(quic.handle(false), ControlResponse::Applied);
        assert_eq!(
            quic.state(),
            ControlResponse::QuicState {
                enabled: false,
                active: false
            }
        );
        let calls = calls.lock().unwrap();
        assert!(calls.contains(&format!("{SYSTEMCTL} enable --now {QUIC_UNIT}")));
        assert!(calls.contains(&format!("{SYSTEMCTL} disable --now {QUIC_UNIT}")));
    }

    #[test]
    fn a_unit_that_cannot_be_switched_is_reported() {
        let mut quic = QuicSettings::open(Box::new(Recorder {
            calls: Arc::new(Mutex::new(Vec::new())),
            enabled: Arc::new(Mutex::new(false)),
            fail: true,
        }));
        assert_eq!(
            quic.handle(true),
            ControlResponse::Rejected {
                code: ControlRejectionCode::BackendRejected
            }
        );
        assert_eq!(
            quic.state(),
            ControlResponse::QuicState {
                enabled: false,
                active: false
            }
        );
    }
}
