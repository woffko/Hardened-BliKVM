//! Owner side of the administration feature switches.
//!
//! The owner is the only writer of the switch file. Helpers and the web
//! server read it and refuse a feature that is off, so a switch set here is
//! enforced even if a client ignores the state it reads back.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};
use bli_kvm_core::{AdminFeature, AdminFeatureSwitches};

pub struct AdminFeaturesState {
    path: PathBuf,
}

impl AdminFeaturesState {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn state(&self) -> ControlResponse {
        let switches = AdminFeatureSwitches::load(&self.path);
        ControlResponse::AdminFeatureState {
            terminal: switches.terminal,
            scripts: switches.scripts,
            remote_download: switches.remote_download,
            picoclaw: switches.picoclaw,
            raw_system_update: switches.raw_system_update,
        }
    }

    pub fn handle(&mut self, feature: AdminFeature, enabled: bool) -> ControlResponse {
        let mut switches = AdminFeatureSwitches::load(&self.path);
        if switches.get(feature) == enabled {
            return ControlResponse::Applied;
        }
        switches.set(feature, enabled);
        match write_switches(&self.path, &switches) {
            Ok(()) => {
                eprintln!(
                    "admin features: {} switched {}",
                    feature.key(),
                    if enabled { "on" } else { "off" }
                );
                ControlResponse::Applied
            }
            Err(reason) => {
                eprintln!("admin features: switch write failed: {reason}");
                ControlResponse::Rejected {
                    code: ControlRejectionCode::BackendRejected,
                }
            }
        }
    }
}

fn write_switches(path: &Path, switches: &AdminFeatureSwitches) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "the switch file has no parent directory".to_string())?;
    let metadata = std::fs::symlink_metadata(parent)
        .map_err(|error| format!("the switch directory is unavailable: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("the switch directory must be a real directory".into());
    }
    let temporary = path.with_extension("json.new");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)
        .map_err(|error| format!("the temporary switch file is unavailable: {error}"))?;
    file.write_all(switches.render().as_bytes())
        .map_err(|error| format!("the temporary switch file could not be written: {error}"))?;
    file.set_permissions(std::fs::Permissions::from_mode(0o644))
        .map_err(|error| format!("the switch file mode could not be set: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("the temporary switch file could not be synced: {error}"))?;
    drop(file);
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("the switch file could not be replaced: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_persist_and_start_off() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("admin-features.json");
        let mut state = AdminFeaturesState::open(&path);
        assert_eq!(
            state.state(),
            ControlResponse::AdminFeatureState {
                terminal: false,
                scripts: false,
                remote_download: false,
                picoclaw: false,
                raw_system_update: false,
            }
        );
        assert_eq!(
            state.handle(AdminFeature::Scripts, true),
            ControlResponse::Applied
        );
        let reopened = AdminFeaturesState::open(&path);
        assert_eq!(
            reopened.state(),
            ControlResponse::AdminFeatureState {
                terminal: false,
                scripts: true,
                remote_download: false,
                picoclaw: false,
                raw_system_update: false,
            }
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o644);
        assert_eq!(
            state.handle(AdminFeature::Scripts, false),
            ControlResponse::Applied
        );
        assert!(!AdminFeatureSwitches::load(&path).scripts);
        // A missing directory is refused rather than silently ignored.
        let mut broken = AdminFeaturesState::open(dir.path().join("missing/admin-features.json"));
        assert!(matches!(
            broken.handle(AdminFeature::Terminal, true),
            ControlResponse::Rejected { .. }
        ));
    }
}
