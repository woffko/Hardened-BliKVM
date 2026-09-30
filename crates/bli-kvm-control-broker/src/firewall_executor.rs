//! Native mode executor: semantic observation and handle-bound atomic replacement.
use crate::firewall_confirmation::{PendingFileStore, PendingStore};
use crate::{
    firewall_observation,
    firewall_rules::{BackendIsolation, FirewallMode, FirewallPorts, render},
    firewall_settings::{FirewallRunner, NFT},
    firewall_transaction::ModeExecutor,
};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModeConfiguration {
    #[serde(default)]
    pub https_probe: Option<crate::firewall_health::HttpsProbe>,
    pub https_enabled: bool,
    pub https_port: u16,
    pub syslog_port: u16,
    pub backend_isolation: Option<(String, u16)>,
    #[serde(default)]
    pub webrtc_media: Option<(u16, u16)>,
}

impl ModeConfiguration {
    pub fn read_reviewed(path: &std::path::Path) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err("firewall configuration path must be absolute".into());
        }
        let parent = path.parent().ok_or("missing configuration directory")?;
        PendingFileStore::new(parent)?.validate_directory()?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|error| error.to_string())?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.permissions().mode() & 0o022 != 0
            || metadata.len() > 4096
        {
            return Err("invalid firewall configuration file".into());
        }
        let mut text = String::new();
        file.take(4097)
            .read_to_string(&mut text)
            .map_err(|error| error.to_string())?;
        if text.len() > 4096 {
            return Err("oversized firewall configuration".into());
        }
        let configuration: Self =
            serde_json::from_str(&text).map_err(|_| "invalid firewall configuration JSON")?;
        render(FirewallMode::Baseline, &configuration.ports())?;
        if let Some(probe) = &configuration.https_probe {
            probe.validate()?;
        }
        Ok(configuration)
    }

    fn ports(&self) -> FirewallPorts<'_> {
        FirewallPorts {
            https_enabled: self.https_enabled,
            https_port: self.https_port,
            syslog_port: self.syslog_port,
            backend_isolation: self
                .backend_isolation
                .as_ref()
                .map(|(interface, source_port)| BackendIsolation {
                    interface,
                    source_port: *source_port,
                }),
            webrtc_media: self.webrtc_media,
        }
    }
}

pub struct NftModeExecutor {
    runner: Box<dyn FirewallRunner>,
    state_dir: PathBuf,
    configuration: ModeConfiguration,
    configuration_drift: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredConfiguration {
    schema_version: u8,
    configuration: ModeConfiguration,
}

impl NftModeExecutor {
    /// Configuration must be persisted/frozen by the owner while a transaction
    /// is pending; callers may not reconstruct rollback from changed ports.
    fn new(
        runner: Box<dyn FirewallRunner>,
        state_dir: PathBuf,
        configuration: ModeConfiguration,
    ) -> Result<Self, String> {
        render(FirewallMode::Baseline, &configuration.ports())?;
        Ok(Self {
            runner,
            state_dir,
            configuration,
            configuration_drift: false,
        })
    }

    /// Persisted configuration remains authoritative across restart. A caller
    /// must resolve drift explicitly before accepting new mode changes; pending
    /// rollback must always use these original inputs.
    pub fn open_frozen(
        runner: Box<dyn FirewallRunner>,
        state_dir: PathBuf,
        requested: ModeConfiguration,
    ) -> Result<Self, String> {
        Self::open_inner(runner, state_dir, Some(requested))
    }

    pub fn reopen_for_recovery(
        runner: Box<dyn FirewallRunner>,
        state_dir: PathBuf,
    ) -> Result<Self, String> {
        Self::open_inner(runner, state_dir, None)
    }

    fn open_inner(
        mut runner: Box<dyn FirewallRunner>,
        state_dir: PathBuf,
        requested: Option<ModeConfiguration>,
    ) -> Result<Self, String> {
        let mut guard = PendingFileStore::new(&state_dir)?;
        guard.validate_directory()?;
        let path = state_dir.join("mode-config.json");
        let configuration = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
        {
            Ok(file) => {
                let metadata = file.metadata().map_err(|error| error.to_string())?;
                if !metadata.is_file()
                    || metadata.permissions().mode() & 0o077 != 0
                    || metadata.nlink() != 1
                    || metadata.len() > 4096
                {
                    return Err("invalid frozen firewall configuration file".into());
                }
                let mut content = String::new();
                file.take(4097)
                    .read_to_string(&mut content)
                    .map_err(|error| error.to_string())?;
                if content.len() > 4096 {
                    return Err("oversized frozen firewall configuration".into());
                }
                let stored: StoredConfiguration = serde_json::from_str(&content)
                    .map_err(|_| "invalid frozen firewall configuration")?;
                if stored.schema_version != 1 {
                    return Err("unsupported firewall configuration version".into());
                }
                stored.configuration
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let requested = requested
                    .as_ref()
                    .ok_or("frozen firewall configuration is missing")?;
                if guard.load()?.is_some() {
                    return Err("pending firewall change has no frozen configuration".into());
                }
                render(FirewallMode::Baseline, &requested.ports())?;
                let content = serde_json::to_string(&StoredConfiguration {
                    schema_version: 1,
                    configuration: requested.clone(),
                })
                .map_err(|error| error.to_string())?;
                runner.write_file(&path, &content)?;
                requested.clone()
            }
            Err(error) => return Err(error.to_string()),
        };
        let drift = requested
            .as_ref()
            .is_none_or(|requested| &configuration != requested);
        let mut executor = Self::new(runner, state_dir, configuration)?;
        executor.configuration_drift = drift;
        Ok(executor)
    }

    pub fn configuration_matches(&self, requested: &ModeConfiguration) -> bool {
        &self.configuration == requested
    }

    fn observed_table(&mut self) -> Result<Option<(FirewallMode, u64)>, String> {
        let inventory = self.runner.run(NFT, &["-j", "list", "tables"])?;
        if inventory.len() > 65536 {
            return Err("oversized table inventory".into());
        }
        let value: serde_json::Value =
            serde_json::from_str(&inventory).map_err(|_| "invalid table inventory")?;
        let entries = value
            .get("nftables")
            .and_then(serde_json::Value::as_array)
            .ok_or("missing table inventory")?;
        let mut observed = None;
        for entry in entries {
            if let Some(table) = entry.get("table") {
                if table["family"] != "inet" || table["name"] != "hardened-blikvm" {
                    continue;
                }
                if observed.is_some() {
                    return Err("duplicate owned table".into());
                }
                let (comment, source) = match table.get("comment") {
                    Some(comment) => (
                        comment.as_str().map(str::to_owned),
                        firewall_observation::CommentSource::Json,
                    ),
                    None => {
                        let listing = self
                            .runner
                            .run(NFT, &["list", "table", "inet", "hardened-blikvm"])?;
                        (
                            firewall_observation::text_table_comment(&listing).map(str::to_owned),
                            firewall_observation::CommentSource::Text,
                        )
                    }
                };
                let mode = match comment.as_deref() {
                    Some("hardened-blikvm-owner-v2:baseline") => FirewallMode::Baseline,
                    Some("hardened-blikvm-owner-v2:moderate") => FirewallMode::Moderate,
                    Some("hardened-blikvm-owner-v2:restricted") => FirewallMode::Restricted,
                    Some("hardened-blikvm-owner-v2:paranoid") => FirewallMode::Paranoid,
                    _ => return Err("unrecognized firewall ownership/version".into()),
                };
                let handle = table["handle"]
                    .as_u64()
                    .filter(|handle| *handle > 0)
                    .ok_or("invalid table handle")?;
                observed = Some((mode, handle, source));
            }
        }
        if let Some((mode, handle, source)) = observed {
            let document = self
                .runner
                .run(NFT, &["-j", "list", "table", "inet", "hardened-blikvm"])?;
            if firewall_observation::verify_with(
                &document,
                mode,
                &self.configuration.ports(),
                source,
            )? != handle
            {
                return Err("firewall changed during observation".into());
            }
        }
        Ok(observed.map(|(mode, handle, _)| (mode, handle)))
    }
}

impl ModeExecutor for NftModeExecutor {
    fn preflight(&mut self, mode: FirewallMode) -> Result<(), String> {
        if !mode.requires_confirmation() {
            return Ok(());
        }
        if !self.configuration.https_enabled {
            return Err("HTTPS is disabled".into());
        }
        let probe = self
            .configuration
            .https_probe
            .as_ref()
            .ok_or("verified HTTPS probe is not configured")?;
        probe.check(self.runner.as_mut(), self.configuration.https_port)
    }
    fn admit_change(&self) -> Result<(), String> {
        if self.configuration_drift {
            Err("firewall configuration drift requires reconciliation".into())
        } else {
            Ok(())
        }
    }
    fn observe(&mut self) -> Result<FirewallMode, String> {
        match self.observed_table()? {
            Some((mode, _)) => Ok(mode),
            None if self.configuration.backend_isolation.is_none() => Ok(FirewallMode::Baseline),
            None => Err("required backend isolation is absent".into()),
        }
    }

    fn apply(&mut self, mode: FirewallMode) -> Result<(), String> {
        let generated = render(mode, &self.configuration.ports())?;
        let observed = self.observed_table()?;
        if observed.is_some_and(|(current, _)| current == mode) {
            return Ok(());
        }
        let batch = match observed {
            Some((_, handle)) => format!("delete table inet handle {handle}\n{generated}"),
            None => generated,
        };
        let path = self.state_dir.join("mode-candidate.nft");
        self.runner.write_file(&path, &batch)?;
        let applied = self.runner.run(NFT, &["-f", &path.to_string_lossy()]);
        let _ = self.runner.remove_file(&path);
        applied?;
        match self.observed_table()? {
            Some((current, _)) if current == mode => Ok(()),
            _ => Err("firewall mode read-back mismatch".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firewall_confirmation::PendingChange;
    use crate::firewall_settings::SystemFirewallRunner;

    fn configuration() -> ModeConfiguration {
        ModeConfiguration {
            https_enabled: true,
            https_port: 8443,
            syslog_port: 1514,
            backend_isolation: None,
            webrtc_media: None,
            https_probe: None,
        }
    }

    #[test]
    fn reviewed_configuration_has_no_guessed_ports_or_unsafe_file_fallback() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("reviewed.json");
        let text = serde_json::to_string(&configuration()).unwrap();
        SystemFirewallRunner.write_file(&path, &text).unwrap();
        assert_eq!(
            ModeConfiguration::read_reviewed(&path).unwrap(),
            configuration()
        );
        SystemFirewallRunner
            .write_file(&path, "{\"httpsEnabled\":true}")
            .unwrap();
        assert!(ModeConfiguration::read_reviewed(&path).is_err());
        SystemFirewallRunner.write_file(&path, &text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(ModeConfiguration::read_reviewed(&path).is_err());
    }

    #[test]
    fn missing_configuration_cannot_be_recreated_over_pending_recovery() {
        let root = tempfile::tempdir().unwrap();
        let pending = PendingChange::prepare(
            [1; 16],
            [2; 32],
            [3; 16],
            FirewallMode::Moderate,
            FirewallMode::Restricted,
            100,
        )
        .unwrap();
        PendingFileStore::new(root.path())
            .unwrap()
            .save(&pending.encode().unwrap())
            .unwrap();
        assert!(
            NftModeExecutor::open_frozen(
                Box::new(SystemFirewallRunner),
                root.path().into(),
                configuration()
            )
            .is_err()
        );
        assert!(!root.path().join("mode-config.json").exists());
    }

    #[test]
    fn malformed_and_symlinked_configurations_are_not_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("mode-config.json");
        SystemFirewallRunner.write_file(&path, "{}").unwrap();
        assert!(
            NftModeExecutor::open_frozen(
                Box::new(SystemFirewallRunner),
                root.path().into(),
                configuration()
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(root.path().join("missing"), &path).unwrap();
        assert!(
            NftModeExecutor::open_frozen(
                Box::new(SystemFirewallRunner),
                root.path().into(),
                configuration()
            )
            .is_err()
        );
        assert!(!root.path().join("missing").exists());
    }
}
