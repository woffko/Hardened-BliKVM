//! Reviewed video-settings application for the privileged board owner.
//!
//! The desired frame rate and JPEG quality are start-up options of the retained
//! video engine, so applying them means writing the owner-controlled
//! environment file its unit reads and restarting that unit. The board owner is
//! the only component with the privilege to do it, and it keeps the operation
//! reversible: a restart that fails restores the previous environment file.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};
use bli_kvm_core::{VideoSettings, VideoSettingsChange};

/// Restarts the video-engine unit. The runner is injected so a test can assert
/// the exact unit name without touching the device.
pub trait VideoRestartRunner: Send {
    fn restart(&mut self, unit: &str) -> Result<(), String>;
}

/// Runs `systemctl restart <unit>`, bounded by the unit's own start timeout.
pub struct ExecVideoRestartRunner;

impl VideoRestartRunner for ExecVideoRestartRunner {
    fn restart(&mut self, unit: &str) -> Result<(), String> {
        let status = std::process::Command::new("/usr/bin/systemctl")
            .args(["restart", unit])
            .status()
            .map_err(|error| error.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("systemctl restart {unit} exited with {status}"))
        }
    }
}

pub struct VideoSettingsState {
    env_path: PathBuf,
    unit: String,
    runner: Box<dyn VideoRestartRunner>,
    current: VideoSettings,
    capability_enabled: bool,
    backend_ready: bool,
}

impl VideoSettingsState {
    pub fn open(
        env_path: impl Into<PathBuf>,
        unit: impl Into<String>,
        runner: Box<dyn VideoRestartRunner>,
        capability_enabled: bool,
        backend_ready: bool,
    ) -> Self {
        let env_path = env_path.into();
        let current = std::fs::read_to_string(&env_path)
            .map(|text| VideoSettings::from_env_file(&text))
            .unwrap_or_default();
        Self {
            env_path,
            unit: unit.into(),
            runner,
            current,
            capability_enabled,
            backend_ready,
        }
    }

    pub fn current(&self) -> VideoSettings {
        self.current.clone()
    }

    pub fn handle(&mut self, change: &VideoSettingsChange) -> ControlResponse {
        if !self.capability_enabled {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        if !self.backend_ready {
            return reject(ControlRejectionCode::BackendUnavailable);
        }
        if change.validate().is_err() {
            return reject(ControlRejectionCode::InvalidState);
        }
        let next = match self.current.apply(change) {
            Ok(next) => next,
            Err(_) => return reject(ControlRejectionCode::InvalidState),
        };
        if next == self.current {
            // The requested state is already in force.
            return ControlResponse::Applied;
        }
        if let Err(reason) = self.write_env_file(&next) {
            eprintln!("video settings: environment file write failed: {reason}");
            return reject(ControlRejectionCode::BackendRejected);
        }
        if let Err(reason) = self.runner.restart(&self.unit) {
            eprintln!("video settings: unit restart failed: {reason}");
            // Reversibility: the engine keeps running the previous settings.
            if let Err(restore) = self.write_env_file(&self.current) {
                eprintln!("video settings: environment file restore failed: {restore}");
            }
            return reject(ControlRejectionCode::BackendRejected);
        }
        self.current = next;
        ControlResponse::Applied
    }

    fn write_env_file(&self, settings: &VideoSettings) -> Result<(), String> {
        let parent = self
            .env_path
            .parent()
            .ok_or_else(|| "the environment file has no parent directory".to_string())?;
        let metadata = std::fs::symlink_metadata(parent)
            .map_err(|error| format!("the environment directory is unavailable: {error}"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("the environment directory must be a real directory".into());
        }
        let temporary = self.env_path.with_extension("env.new");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .map_err(|error| format!("the temporary environment file is unavailable: {error}"))?;
        file.write_all(settings.to_env_file().as_bytes())
            .map_err(|error| {
                format!("the temporary environment file could not be written: {error}")
            })?;
        file.sync_all().map_err(|error| {
            format!("the temporary environment file could not be synced: {error}")
        })?;
        // The mode is fixed so the engine user can only read it.
        std::fs::set_permissions(
            &temporary,
            std::os::unix::fs::PermissionsExt::from_mode(0o644),
        )
        .map_err(|error| format!("the environment file mode could not be set: {error}"))?;
        drop(file);
        std::fs::rename(&temporary, &self.env_path)
            .map_err(|error| format!("the environment file could not be replaced: {error}"))
    }

    /// The path the owner writes and the unit reads, for tests and diagnostics.
    pub fn env_path(&self) -> &Path {
        &self.env_path
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

    fn state(
        env_path: &Path,
        fail: bool,
        enabled: bool,
    ) -> (VideoSettingsState, Arc<Mutex<Vec<String>>>) {
        let restarts = Arc::new(Mutex::new(Vec::new()));
        let runner = RecordingRunner {
            restarts: restarts.clone(),
            fail,
        };
        (
            VideoSettingsState::open(
                env_path,
                "hardened-blikvm-ustreamer.service",
                Box::new(runner),
                enabled,
                true,
            ),
            restarts,
        )
    }

    #[test]
    fn applies_a_change_by_writing_the_environment_and_restarting_the_unit() {
        let directory = tempfile::tempdir().unwrap();
        let env_path = directory.path().join("ustreamer.env");
        let (mut state, restarts) = state(&env_path, false, true);

        assert_eq!(
            state.handle(&VideoSettingsChange::new(Some(24), None)),
            ControlResponse::Applied
        );
        let written = std::fs::read_to_string(&env_path).unwrap();
        assert!(written.contains("BLI_KVM_USTREAMER_FPS=24"));
        assert!(written.contains("BLI_KVM_USTREAMER_QUALITY=80"));
        assert_eq!(state.current().fps, 24);
        assert_eq!(
            restarts.lock().unwrap().as_slice(),
            ["hardened-blikvm-ustreamer.service"]
        );

        // A change back to the current state is applied without a restart.
        assert_eq!(
            state.handle(&VideoSettingsChange::new(Some(24), None)),
            ControlResponse::Applied
        );
        assert_eq!(restarts.lock().unwrap().len(), 1);
    }

    #[test]
    fn refuses_out_of_range_or_disabled_requests_without_touching_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let env_path = directory.path().join("ustreamer.env");
        let (mut disabled, restarts) = state(&env_path, false, false);
        assert_eq!(
            disabled.handle(&VideoSettingsChange::new(Some(30), None)),
            ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled
            }
        );
        assert!(!env_path.exists());
        assert!(restarts.lock().unwrap().is_empty());

        let (mut enabled, restarts) = state(&env_path, false, true);
        assert_eq!(
            enabled.handle(&VideoSettingsChange::new(Some(0), None)),
            ControlResponse::Rejected {
                code: ControlRejectionCode::InvalidState
            }
        );
        assert_eq!(
            enabled.handle(&VideoSettingsChange::new(None, Some(101))),
            ControlResponse::Rejected {
                code: ControlRejectionCode::InvalidState
            }
        );
        assert_eq!(
            enabled.handle(&VideoSettingsChange::new(None, None)),
            ControlResponse::Rejected {
                code: ControlRejectionCode::InvalidState
            }
        );
        assert!(!env_path.exists());
        assert!(restarts.lock().unwrap().is_empty());
    }

    #[test]
    fn restores_the_previous_file_when_the_restart_fails() {
        let directory = tempfile::tempdir().unwrap();
        let env_path = directory.path().join("ustreamer.env");
        let (mut state, restarts) = state(&env_path, true, true);
        assert_eq!(
            state.handle(&VideoSettingsChange::new(Some(30), Some(100))),
            ControlResponse::Rejected {
                code: ControlRejectionCode::BackendRejected
            }
        );
        assert!(restarts.lock().unwrap().is_empty());
        // The engine still runs the reviewed default and the file says so.
        assert_eq!(state.current(), VideoSettings::default());
        let written = std::fs::read_to_string(&env_path).unwrap();
        assert_eq!(
            VideoSettings::from_env_file(&written),
            VideoSettings::default()
        );
    }

    #[test]
    fn loads_the_previous_state_from_the_environment_file() {
        let directory = tempfile::tempdir().unwrap();
        let env_path = directory.path().join("ustreamer.env");
        std::fs::write(
            &env_path,
            "BLI_KVM_USTREAMER_FPS=30\nBLI_KVM_USTREAMER_QUALITY=50\n",
        )
        .unwrap();
        let (state, _) = state(&env_path, false, true);
        assert_eq!(state.current(), VideoSettings::new(30, 50).unwrap());
        assert_eq!(state.env_path(), env_path.as_path());
    }
}
