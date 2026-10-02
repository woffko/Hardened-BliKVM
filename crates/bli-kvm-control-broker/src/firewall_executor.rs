//! Native mode executor: semantic observation and handle-bound atomic replacement.
use crate::firewall_confirmation::{PendingFileStore, PendingStore};
use crate::{
    firewall_observation,
    firewall_rules::{BackendIsolation, FirewallMode, FirewallPorts, RuleForm, render},
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

/// The owned table as the kernel holds it: its mode, its handle, and which
/// build's rules for that mode it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Observed {
    mode: FirewallMode,
    handle: u64,
    form: RuleForm,
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

    fn observed_table(&mut self) -> Result<Option<Observed>, String> {
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
        let Some((mode, handle, source)) = observed else {
            return Ok(None);
        };
        let document = self
            .runner
            .run(NFT, &["-j", "list", "table", "inet", "hardened-blikvm"])?;
        let ports = self.configuration.ports();
        // The table is this build's, or what an earlier build wrote for the
        // same mode; anything else is not ours to interpret.
        let mut refusal = "nft rule content mismatch";
        for form in RuleForm::of(mode) {
            match firewall_observation::verify_form(&document, mode, &ports, source, *form) {
                Ok(verified) if verified == handle => {
                    return Ok(Some(Observed {
                        mode,
                        handle,
                        form: *form,
                    }));
                }
                Ok(_) => return Err("firewall changed during observation".into()),
                Err(error) if *form == RuleForm::Current => refusal = error,
                Err(_) => {}
            }
        }
        Err(refusal.into())
    }

    /// Replaces the owned table, if there is one, by the rules of `mode` as
    /// this build writes them, in one nft transaction.
    fn replace(&mut self, mode: FirewallMode, observed: Option<Observed>) -> Result<(), String> {
        let generated = render(mode, &self.configuration.ports())?;
        let batch = match observed {
            Some(table) => format!("delete table inet handle {}\n{generated}", table.handle),
            None => generated,
        };
        let path = self.state_dir.join("mode-candidate.nft");
        self.runner.write_file(&path, &batch)?;
        let applied = self.runner.run(NFT, &["-f", &path.to_string_lossy()]);
        let _ = self.runner.remove_file(&path);
        applied?;
        match self.observed_table()? {
            Some(table) if table.mode == mode && table.form == RuleForm::Current => Ok(()),
            _ => Err("firewall mode read-back mismatch".into()),
        }
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
            Some(table) => Ok(table.mode),
            None if self.configuration.backend_isolation.is_none() => Ok(FirewallMode::Baseline),
            None => Err("required backend isolation is absent".into()),
        }
    }

    fn apply(&mut self, mode: FirewallMode) -> Result<(), String> {
        render(mode, &self.configuration.ports())?;
        let observed = self.observed_table()?;
        if observed.is_some_and(|table| table.mode == mode && table.form == RuleForm::Current) {
            return Ok(());
        }
        self.replace(mode, observed)
    }

    fn refresh(&mut self) -> Result<Option<FirewallMode>, String> {
        match self.observed_table()? {
            Some(table) if table.form != RuleForm::Current => {
                self.replace(table.mode, Some(table))?;
                Ok(Some(table.mode))
            }
            _ => Ok(None),
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

    /// nft as the executor meets it: a kernel that holds the owned table in
    /// one of its forms, or no owned table. Files are real ones.
    #[derive(Clone)]
    struct Kernel(std::sync::Arc<std::sync::Mutex<KernelState>>);
    struct KernelState {
        table: Option<(FirewallMode, RuleForm)>,
        handle: u64,
        /// Something the owner never wrote is in its table.
        foreign_rule: bool,
        batches: Vec<String>,
    }

    impl Kernel {
        fn holding(table: Option<(FirewallMode, RuleForm)>) -> Self {
            Self(std::sync::Arc::new(std::sync::Mutex::new(KernelState {
                table,
                handle: 7,
                foreign_rule: false,
                batches: Vec::new(),
            })))
        }
        fn table(&self) -> Option<(FirewallMode, RuleForm)> {
            self.0.lock().unwrap().table
        }
        fn batches(&self) -> Vec<String> {
            self.0.lock().unwrap().batches.clone()
        }
    }

    impl FirewallRunner for Kernel {
        fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
            assert_eq!(program, NFT);
            let mut kernel = self.0.lock().unwrap();
            match args {
                ["-j", "list", "tables"] => {
                    let mut entries =
                        vec![serde_json::json!({"metainfo": {"json_schema_version": 1}})];
                    if let Some((mode, _)) = kernel.table {
                        entries.push(serde_json::json!({"table": {
                            "family": "inet",
                            "name": "hardened-blikvm",
                            "handle": kernel.handle,
                            "comment": format!("hardened-blikvm-owner-v2:{}", mode.as_str()),
                        }}));
                    }
                    Ok(serde_json::json!({ "nftables": entries }).to_string())
                }
                ["-j", "list", "table", "inet", "hardened-blikvm"] => {
                    let (mode, form) = kernel.table.ok_or("no such table")?;
                    let mut document: serde_json::Value =
                        serde_json::from_str(&firewall_observation::observed_document(
                            mode,
                            &configuration().ports(),
                            form,
                        ))
                        .unwrap();
                    document["nftables"][1]["table"]["handle"] = kernel.handle.into();
                    if kernel.foreign_rule {
                        document["nftables"]
                            .as_array_mut()
                            .unwrap()
                            .push(serde_json::json!({
                                "rule": {
                                    "family": "inet",
                                    "table": "hardened-blikvm",
                                    "chain": "input",
                                    "handle": 99,
                                    "expr": [{"accept": null}],
                                }
                            }));
                    }
                    Ok(document.to_string())
                }
                ["-f", path] => {
                    let batch = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
                    let rules = match batch
                        .strip_prefix(&format!("delete table inet handle {}\n", kernel.handle))
                    {
                        Some(rest) => rest,
                        None if kernel.table.is_none() => &batch,
                        None => return Err("the table exists".into()),
                    };
                    let mode = [
                        FirewallMode::Baseline,
                        FirewallMode::Moderate,
                        FirewallMode::Restricted,
                        FirewallMode::Paranoid,
                    ]
                    .into_iter()
                    .find(|mode| rules == render(*mode, &configuration().ports()).unwrap())
                    .ok_or("rules of no mode")?;
                    kernel.table = Some((mode, RuleForm::Current));
                    kernel.handle += 1;
                    kernel.foreign_rule = false;
                    kernel.batches.push(batch);
                    Ok(String::new())
                }
                other => Err(format!("unexpected nft call: {other:?}")),
            }
        }
        fn exists(&mut self, path: &std::path::Path) -> bool {
            SystemFirewallRunner.exists(path)
        }
        fn read_file(&mut self, path: &std::path::Path) -> Result<String, String> {
            SystemFirewallRunner.read_file(path)
        }
        fn write_file(&mut self, path: &std::path::Path, text: &str) -> Result<(), String> {
            SystemFirewallRunner.write_file(path, text)
        }
        fn remove_file(&mut self, path: &std::path::Path) -> Result<(), String> {
            SystemFirewallRunner.remove_file(path)
        }
    }

    fn opened(kernel: &Kernel) -> (tempfile::TempDir, NftModeExecutor) {
        let root = tempfile::tempdir().unwrap();
        let executor = NftModeExecutor::open_frozen(
            Box::new(kernel.clone()),
            root.path().into(),
            configuration(),
        )
        .unwrap();
        (root, executor)
    }

    /// An owner that was updated finds the rules its earlier build wrote.
    /// Restricted then lacks the QUIC transport's port: it is still
    /// Restricted, and its rules are replaced by this build's in one batch.
    #[test]
    fn rules_from_before_the_quic_port_are_restricted_and_are_brought_up_to_date() {
        let kernel = Kernel::holding(Some((FirewallMode::Restricted, RuleForm::BeforeQuicPort)));
        let (root, mut executor) = opened(&kernel);
        assert_eq!(executor.observe(), Ok(FirewallMode::Restricted));
        assert!(kernel.batches().is_empty());

        assert_eq!(executor.refresh(), Ok(Some(FirewallMode::Restricted)));
        assert_eq!(
            kernel.table(),
            Some((FirewallMode::Restricted, RuleForm::Current))
        );
        let batches = kernel.batches();
        assert_eq!(batches.len(), 1);
        // The old table goes and the new one comes in the same transaction.
        assert!(batches[0].starts_with("delete table inet handle 7\ncreate table inet"));
        assert!(batches[0].contains("udp dport 8443 accept"));
        assert!(!root.path().join("mode-candidate.nft").exists());

        // Up to date: nothing more is written, by a refresh or by the mode
        // being asked for again.
        assert_eq!(executor.refresh(), Ok(None));
        assert_eq!(executor.apply(FirewallMode::Restricted), Ok(()));
        assert_eq!(kernel.batches().len(), 1);
        assert_eq!(executor.observe(), Ok(FirewallMode::Restricted));
    }

    #[test]
    fn asking_for_the_mode_in_force_also_replaces_earlier_rules() {
        let kernel = Kernel::holding(Some((FirewallMode::Restricted, RuleForm::BeforeQuicPort)));
        let (_root, mut executor) = opened(&kernel);
        assert_eq!(executor.apply(FirewallMode::Restricted), Ok(()));
        assert_eq!(kernel.batches().len(), 1);
        assert_eq!(
            kernel.table(),
            Some((FirewallMode::Restricted, RuleForm::Current))
        );
        // Leaving the mode works from either form.
        let kernel = Kernel::holding(Some((FirewallMode::Restricted, RuleForm::BeforeQuicPort)));
        let (_root, mut executor) = opened(&kernel);
        assert_eq!(executor.apply(FirewallMode::Paranoid), Ok(()));
        assert_eq!(
            kernel.table(),
            Some((FirewallMode::Paranoid, RuleForm::Current))
        );
    }

    #[test]
    fn current_rules_and_no_table_need_no_refresh() {
        for table in [
            None,
            Some((FirewallMode::Moderate, RuleForm::Current)),
            Some((FirewallMode::Restricted, RuleForm::Current)),
            Some((FirewallMode::Paranoid, RuleForm::Current)),
        ] {
            let kernel = Kernel::holding(table);
            let (_root, mut executor) = opened(&kernel);
            assert_eq!(executor.refresh(), Ok(None), "{table:?}");
            assert!(kernel.batches().is_empty());
            assert_eq!(
                executor.observe(),
                Ok(table.map_or(FirewallMode::Baseline, |(mode, _)| mode))
            );
        }
    }

    #[test]
    fn a_table_that_is_no_form_of_its_mode_is_not_interpreted() {
        for form in [RuleForm::Current, RuleForm::BeforeQuicPort] {
            let kernel = Kernel::holding(Some((FirewallMode::Restricted, form)));
            kernel.0.lock().unwrap().foreign_rule = true;
            let (_root, mut executor) = opened(&kernel);
            assert_eq!(
                executor.observe(),
                Err("nft rule content mismatch".to_owned())
            );
            assert!(executor.refresh().is_err());
            assert!(executor.apply(FirewallMode::Moderate).is_err());
            assert!(kernel.batches().is_empty());
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
