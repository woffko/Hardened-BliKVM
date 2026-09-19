//! Reviewed system-log actions owned by the privileged board owner.
//!
//! The reference can send a test line through its log service. This board has
//! no such service, so the owner writes one bounded line to the journal with
//! `logger`, which is the surface the interface's local view reads.

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};

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
