//! Persisted administration feature switches.
//!
//! The board owner writes the file; the web server and the privileged helpers
//! only read it. Every switch is off unless the file says otherwise, and a
//! missing, oversized or malformed file keeps every feature off.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::control_v2::AdminFeature;

/// Owner-written switch file (root, 0644) in the owner's state directory,
/// which the web server can read through the `hardened-blikvm` group.
pub const ADMIN_FEATURES_PATH: &str = "/var/lib/hardened-blikvm-gadget/admin-features.json";
const MAX_FILE_BYTES: u64 = 4096;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminFeatureSwitches {
    pub terminal: bool,
    pub scripts: bool,
    pub remote_download: bool,
    pub picoclaw: bool,
    pub raw_system_update: bool,
}

impl AdminFeatureSwitches {
    /// Reads the switches; anything but a small, well-formed file means off.
    pub fn load(path: &Path) -> Self {
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            return Self::default();
        };
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            return Self::default();
        }
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn get(self, feature: AdminFeature) -> bool {
        match feature {
            AdminFeature::Terminal => self.terminal,
            AdminFeature::Scripts => self.scripts,
            AdminFeature::RemoteDownload => self.remote_download,
            AdminFeature::Picoclaw => self.picoclaw,
            AdminFeature::RawSystemUpdate => self.raw_system_update,
        }
    }

    pub fn set(&mut self, feature: AdminFeature, enabled: bool) {
        match feature {
            AdminFeature::Terminal => self.terminal = enabled,
            AdminFeature::Scripts => self.scripts = enabled,
            AdminFeature::RemoteDownload => self.remote_download = enabled,
            AdminFeature::Picoclaw => self.picoclaw = enabled,
            AdminFeature::RawSystemUpdate => self.raw_system_update = enabled,
        }
    }

    pub fn render(self) -> String {
        let mut text = serde_json::to_string_pretty(&self).unwrap_or_default();
        text.push('\n');
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_default_off_and_malformed_files_stay_off() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("admin-features.json");
        assert_eq!(
            AdminFeatureSwitches::load(&path),
            AdminFeatureSwitches::default()
        );

        let mut switches = AdminFeatureSwitches::default();
        switches.set(AdminFeature::Terminal, true);
        std::fs::write(&path, switches.render()).unwrap();
        let loaded = AdminFeatureSwitches::load(&path);
        assert!(loaded.get(AdminFeature::Terminal));
        for feature in [
            AdminFeature::Scripts,
            AdminFeature::RemoteDownload,
            AdminFeature::Picoclaw,
            AdminFeature::RawSystemUpdate,
        ] {
            assert!(!loaded.get(feature));
        }

        for bad in [
            "{\"terminal\":true,\"extra\":true}",
            "{\"terminal\":\"yes\"}",
            "not json",
        ] {
            std::fs::write(&path, bad).unwrap();
            assert_eq!(
                AdminFeatureSwitches::load(&path),
                AdminFeatureSwitches::default()
            );
        }
        std::fs::write(&path, " ".repeat(5000)).unwrap();
        assert_eq!(
            AdminFeatureSwitches::load(&path),
            AdminFeatureSwitches::default()
        );
        // Missing keys are off.
        std::fs::write(&path, "{\"scripts\":true}").unwrap();
        let partial = AdminFeatureSwitches::load(&path);
        assert!(partial.scripts && !partial.terminal);
    }
}
