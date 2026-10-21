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

/// Runs `systemctl restart <unit>`, killed past the owner's deadline for
/// system commands.
pub struct ExecVideoRestartRunner;

impl VideoRestartRunner for ExecVideoRestartRunner {
    fn restart(&mut self, unit: &str) -> Result<(), String> {
        let mut command = std::process::Command::new("/usr/bin/systemctl");
        command.args(["restart", unit]);
        crate::firewall_process::run(command, crate::tailscale::SYSTEM_COMMAND_TIMEOUT, 64 * 1024)
            .map(|_| ())
            .map_err(|reason| format!("systemctl restart {unit} failed: {reason}"))
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

    pub fn readback(&self) -> Result<VideoSettings, ControlRejectionCode> {
        if !self.capability_enabled {
            return Err(ControlRejectionCode::CapabilityDisabled);
        }
        if !self.backend_ready {
            return Err(ControlRejectionCode::BackendUnavailable);
        }
        Ok(self.current())
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
            // Restart may already have stopped the previous engine. Restoring
            // the file alone does not restore service availability.
            self.backend_ready = false;
            if let Err(restore) = self.write_env_file(&self.current) {
                eprintln!("video settings: environment file restore failed: {restore}");
            } else if let Err(restore) = self.runner.restart(&self.unit) {
                eprintln!("video settings: previous configuration restart failed: {restore}");
            } else {
                self.backend_ready = true;
            }
            return reject(ControlRejectionCode::BackendRejected);
        }
        self.current = next;
        ControlResponse::Applied
    }

    fn write_env_file(&self, settings: &VideoSettings) -> Result<(), String> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        use std::sync::atomic::{AtomicU64, Ordering};
        let parent = self
            .env_path
            .parent()
            .ok_or_else(|| "the environment file has no parent directory".to_string())?;
        let metadata = std::fs::symlink_metadata(parent)
            .map_err(|error| format!("the environment directory is unavailable: {error}"))?;
        if !metadata.is_dir() || metadata.permissions().mode() & 0o022 != 0 {
            return Err("the environment directory must be a real directory".into());
        }
        for ancestor in parent.ancestors() {
            if !std::fs::symlink_metadata(ancestor)
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                return Err("the environment directory cannot traverse symlinks".into());
            }
        }
        match std::fs::symlink_metadata(&self.env_path) {
            Ok(metadata) if !metadata.is_file() => {
                return Err("the environment file must be regular".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        static SEQUENCE: AtomicU64 = AtomicU64::new(1);
        let temporary = parent.join(format!(
            ".video-env-{}-{}.tmp",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&temporary)
            .map_err(|error| format!("the temporary environment file is unavailable: {error}"))?;
        let result = (|| {
            // Settings contain no secrets and must remain readable by the engine.
            file.set_permissions(std::fs::Permissions::from_mode(0o644))?;
            file.write_all(settings.to_env_file().as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&temporary, &self.env_path)?;
            std::fs::File::open(parent)?.sync_all()
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.map_err(|error: std::io::Error| format!("environment update failed: {error}"))
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
    fn failed_change_restarts_the_restored_configuration() {
        struct FailOnce(Arc<std::sync::atomic::AtomicUsize>);
        impl VideoRestartRunner for FailOnce {
            fn restart(&mut self, _: &str) -> Result<(), String> {
                if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Err("candidate failed".into())
                } else {
                    Ok(())
                }
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut state = VideoSettingsState::open(
            directory.path().join("ustreamer.env"),
            "streamer.service",
            Box::new(FailOnce(calls.clone())),
            true,
            true,
        );
        assert_eq!(
            state.handle(&VideoSettingsChange::new(Some(24), None)),
            reject(ControlRejectionCode::BackendRejected)
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(state.current(), VideoSettings::default());
        assert_eq!(state.readback(), Ok(VideoSettings::default()));
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
        // The file is restored, but failed recovery is not a live engine.
        assert_eq!(state.current(), VideoSettings::default());
        assert_eq!(
            state.readback(),
            Err(ControlRejectionCode::BackendUnavailable)
        );
        assert_eq!(
            state.handle(&VideoSettingsChange::new(Some(24), None)),
            reject(ControlRejectionCode::BackendUnavailable)
        );
        let written = std::fs::read_to_string(&env_path).unwrap();
        assert_eq!(
            VideoSettings::from_env_file(&written),
            VideoSettings::default()
        );
    }

    #[test]
    fn environment_update_never_follows_the_legacy_temporary_symlink() {
        let directory = tempfile::tempdir().unwrap();
        let env_path = directory.path().join("ustreamer.env");
        let victim = directory.path().join("unrelated");
        std::fs::write(&victim, "preserve unrelated content").unwrap();
        std::os::unix::fs::symlink(&victim, env_path.with_extension("env.new")).unwrap();
        let (mut state, _) = state(&env_path, false, true);
        assert_eq!(
            state.handle(&VideoSettingsChange::new(Some(24), None)),
            ControlResponse::Applied
        );
        assert_eq!(
            std::fs::read_to_string(&victim).unwrap(),
            "preserve unrelated content"
        );
        assert!(
            !std::fs::symlink_metadata(env_path)
                .unwrap()
                .file_type()
                .is_symlink()
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
