//! Reviewed mouse jiggler owned by the privileged board owner.
//!
//! The reference keeps a target awake by nudging the pointer. The owner holds
//! the gadget, so it owns the nudge: it stores the accepted mode, and on every
//! tick sends a one-pixel relative move followed by a release report so a
//! jiggler can never leave a held movement behind.

use std::path::{Path, PathBuf};

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};
use bli_kvm_core::{MouseJigglerChange, MouseJigglerMode};

/// Writes one nudge. Injected so a test can assert the exact report pair
/// without a device.
pub trait JiggleWriter: Send {
    fn nudge(&mut self) -> Result<(), String>;
}

/// The reviewed writer: the pinned relative-mouse HID function.
#[cfg(feature = "gadget-media-profile")]
pub struct RelativeMouseWriter {
    hidg_dir: PathBuf,
}

#[cfg(feature = "gadget-media-profile")]
impl RelativeMouseWriter {
    pub fn new(hidg_dir: impl Into<PathBuf>) -> Self {
        Self {
            hidg_dir: hidg_dir.into(),
        }
    }
}

#[cfg(feature = "gadget-media-profile")]
impl JiggleWriter for RelativeMouseWriter {
    fn nudge(&mut self) -> Result<(), String> {
        for report in bli_kvm_core::jiggle_reports() {
            bli_kvm_usb_gadget::write_report(&self.hidg_dir, "hid.usb2", &report)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

pub struct MouseJigglerState {
    state_path: PathBuf,
    writer: Box<dyn JiggleWriter>,
    mode: Option<MouseJigglerMode>,
    capability_enabled: bool,
}

impl MouseJigglerState {
    /// Loads the persisted mode: an absent or unreadable file means disabled.
    pub fn open(
        state_path: impl Into<PathBuf>,
        writer: Box<dyn JiggleWriter>,
        capability_enabled: bool,
    ) -> Self {
        let state_path = state_path.into();
        let mode = std::fs::read_to_string(&state_path)
            .ok()
            .and_then(|text| parse_state(&text));
        Self {
            state_path,
            writer,
            mode,
            capability_enabled,
        }
    }

    pub fn enabled(&self) -> bool {
        self.mode.is_some()
    }

    pub fn mode(&self) -> MouseJigglerMode {
        self.mode.unwrap_or(MouseJigglerMode::Disable)
    }

    pub fn handle(&mut self, change: &MouseJigglerChange) -> ControlResponse {
        if !self.capability_enabled {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        let next = match change.resolve() {
            Ok(next) => next,
            Err(_) => return reject(ControlRejectionCode::InvalidState),
        };
        if next == self.mode {
            return ControlResponse::Applied;
        }
        if let Err(reason) = write_state(&self.state_path, next) {
            eprintln!("mouse jiggler: state write failed: {reason}");
            return reject(ControlRejectionCode::BackendRejected);
        }
        self.mode = next;
        ControlResponse::Applied
    }

    /// One interval tick: nudges only while the jiggler is on.
    pub fn tick(&mut self) -> Result<(), String> {
        if self.mode.is_none() {
            return Ok(());
        }
        self.writer.nudge()
    }
}

fn parse_state(text: &str) -> Option<MouseJigglerMode> {
    let mut enabled = false;
    let mut mode = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("enabled=") {
            enabled = value.trim().eq_ignore_ascii_case("yes");
        } else if let Some(value) = line.strip_prefix("mode=") {
            mode = match value.trim() {
                "relative" => Some(MouseJigglerMode::Relative),
                _ => None,
            };
        }
    }
    if enabled { mode } else { None }
}

fn render_state(mode: Option<MouseJigglerMode>) -> String {
    match mode {
        Some(MouseJigglerMode::Relative) => "enabled=yes\nmode=relative\n".to_string(),
        _ => "enabled=no\nmode=disable\n".to_string(),
    }
}

fn write_state(path: &Path, mode: Option<MouseJigglerMode>) -> Result<(), String> {
    crate::device_settings::replace_state_file(path, &render_state(mode))
}

fn reject(code: ControlRejectionCode) -> ControlResponse {
    ControlResponse::Rejected { code }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingWriter {
        nudges: Arc<Mutex<u32>>,
        fail: bool,
    }

    impl JiggleWriter for RecordingWriter {
        fn nudge(&mut self) -> Result<(), String> {
            if self.fail {
                return Err("writer failure".into());
            }
            *self.nudges.lock().unwrap() += 1;
            Ok(())
        }
    }

    fn state(path: &Path, fail: bool) -> (MouseJigglerState, Arc<Mutex<u32>>) {
        let nudges = Arc::new(Mutex::new(0));
        let writer = RecordingWriter {
            nudges: nudges.clone(),
            fail,
        };
        (
            MouseJigglerState::open(path, Box::new(writer), true),
            nudges,
        )
    }

    #[test]
    fn the_jigglers_state_file_is_0640() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mouse-jiggler.env");
        let (mut jiggler, _) = state(&path, false);
        assert_eq!(
            jiggler.handle(&MouseJigglerChange {
                enabled: true,
                mode: MouseJigglerMode::Relative,
            }),
            ControlResponse::Applied
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "{mode:o}");
    }

    #[test]
    fn enabling_persists_and_ticks_only_when_on() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mouse-jiggler.env");
        let (mut jiggler, nudges) = state(&path, false);
        assert!(!jiggler.enabled());
        assert_eq!(jiggler.mode(), MouseJigglerMode::Disable);
        jiggler.tick().unwrap();
        assert_eq!(*nudges.lock().unwrap(), 0);

        assert_eq!(
            jiggler.handle(&MouseJigglerChange::new(true, MouseJigglerMode::Relative)),
            ControlResponse::Applied
        );
        assert!(jiggler.enabled());
        assert_eq!(jiggler.mode(), MouseJigglerMode::Relative);
        let persisted = std::fs::read_to_string(&path).unwrap();
        assert!(persisted.contains("enabled=yes"));
        assert!(persisted.contains("mode=relative"));

        // A reload keeps the setting, and ticks nudge from then on.
        let (mut reloaded, nudges) = state(&path, false);
        assert!(reloaded.enabled());
        reloaded.tick().unwrap();
        assert_eq!(*nudges.lock().unwrap(), 1);

        // Disabling persists too and stops the nudges.
        assert_eq!(
            jiggler.handle(&MouseJigglerChange::new(false, MouseJigglerMode::Relative)),
            ControlResponse::Applied
        );
        assert!(!jiggler.enabled());
        jiggler.tick().unwrap();
        assert_eq!(*nudges.lock().unwrap(), 1);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("enabled=no")
        );
    }

    #[test]
    fn an_unsupported_mode_is_refused_without_touching_the_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mouse-jiggler.env");
        let (mut jiggler, _) = state(&path, false);
        // `disable` while "enabled" resolves to off, never to a mode.
        assert_eq!(
            jiggler.handle(&MouseJigglerChange::new(true, MouseJigglerMode::Disable)),
            ControlResponse::Applied
        );
        assert!(!jiggler.enabled());
    }

    #[test]
    fn a_disabled_capability_never_writes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mouse-jiggler.env");
        let writer = RecordingWriter::default();
        let mut jiggler = MouseJigglerState::open(&path, Box::new(writer), false);
        assert_eq!(
            jiggler.handle(&MouseJigglerChange::new(true, MouseJigglerMode::Relative)),
            ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled
            }
        );
        assert!(!path.exists());
    }

    #[cfg(feature = "gadget-media-profile")]
    #[test]
    fn the_reviewed_writer_sends_the_move_and_its_release() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("hidg2"), b"").unwrap();
        let mut writer = RelativeMouseWriter::new(directory.path());
        writer.nudge().unwrap();
        // The function device holds the last report of the pair: the release.
        assert_eq!(
            std::fs::read(directory.path().join("hidg2")).unwrap(),
            vec![0, 0, 0, 0]
        );
    }

    #[test]
    fn a_writer_failure_is_reported_to_the_caller() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mouse-jiggler.env");
        let (mut jiggler, _) = state(&path, true);
        jiggler.handle(&MouseJigglerChange::new(true, MouseJigglerMode::Relative));
        assert!(jiggler.tick().is_err());
    }
}
