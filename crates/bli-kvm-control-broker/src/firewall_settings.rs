//! Reviewed firewall baseline owned by the privileged board owner.
//!
//! The board ships no `nft`, so the surface stays fail-closed: the owner probes
//! the reviewed backends and reports the baseline as unavailable, and every
//! request is refused while no backend exists. When a backend is installed the
//! baseline is one exclusively created, marked table applied with `nft -f`.
//! The previous ruleset is retained, but rollback deletes only the owned table;
//! replaying unrelated tables would duplicate or overwrite host rules.
//!
//! The owner runs in its own network namespace, so `nft` there would edit that
//! namespace instead of the host. Each `nft` command is therefore executed by
//! a short-lived systemd unit the owner asks PID1 to start: the transient unit
//! shares the host network namespace and holds only `CAP_NET_ADMIN`, while the
//! long-running owner keeps its reviewed sandbox.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};

pub const NFT: &str = "/usr/sbin/nft";
pub const IPTABLES: &str = "/usr/sbin/iptables";
pub const IP6TABLES: &str = "/usr/sbin/ip6tables";
/// Where the owner keeps the previous ruleset while it applies a new one.
pub const FIREWALL_STATE_DIR: &str = "/run/hardened-blikvm-firewall";
const MAX_RULESET_BYTES: usize = 64 * 1024;
const OWNER_COMMENT: &str = "hardened-blikvm-owner-v1";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackendView {
    pub installed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FirewallView {
    pub backend_available: bool,
    pub baseline_active: bool,
    pub mode: String,
    pub https_port: u16,
    pub message: String,
    pub nft: BackendView,
    pub iptables: BackendView,
    pub ip6tables: BackendView,
    pub rules_ipv4: String,
    pub rules_ipv6: String,
    pub rules_nft: String,
}

/// Runs the reviewed probes and ruleset commands. Injected so a test asserts
/// the probe, the apply and the restore without a device.
pub trait FirewallRunner: Send {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String>;
    /// Whether a reviewed backend binary exists; the owner must not execute
    /// `nft` inside its own sandbox just to answer that.
    fn exists(&mut self, path: &Path) -> bool;
    fn read_file(&mut self, path: &Path) -> Result<String, String>;
    fn write_file(&mut self, path: &Path, text: &str) -> Result<(), String>;
    fn remove_file(&mut self, path: &Path) -> Result<(), String>;
}

pub struct SystemFirewallRunner;

pub const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";

impl SystemFirewallRunner {
    /// Run `nft` in a short-lived host-network unit so the baseline applies to
    /// the host and not to the owner's private namespace.
    fn run_host(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
        static SEQUENCE: AtomicU64 = AtomicU64::new(1);
        let unit = format!(
            "hardened-blikvm-firewall-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let mut command = Command::new(SYSTEMD_RUN);
        command
            .arg("--quiet")
            .arg("--collect")
            .arg("--wait")
            .arg("--no-ask-password")
            .arg("--property=TimeoutStartSec=5s")
            .arg("--property=RuntimeMaxSec=5s")
            .arg("--property=TimeoutStopSec=1s")
            // Forward the transient service's stdout/stderr to these pipes;
            // --wait alone only waits for completion and leaves output in the journal.
            .arg("--pipe")
            .arg(format!("--unit={unit}"))
            .arg("--property=NoNewPrivileges=yes")
            .arg("--property=PrivateDevices=yes")
            .arg("--property=ProtectSystem=strict")
            .arg(if program == NFT {
                "--property=RestrictAddressFamilies=AF_NETLINK"
            } else {
                "--property=RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX"
            })
            .arg(if program == NFT {
                "--property=CapabilityBoundingSet=CAP_NET_ADMIN"
            } else {
                "--property=CapabilityBoundingSet="
            })
            .arg("--")
            .arg(program)
            .args(args);
        crate::firewall_process::run(
            command,
            std::time::Duration::from_secs(12),
            MAX_RULESET_BYTES,
        )
    }
}

impl FirewallRunner for SystemFirewallRunner {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
        if program == NFT || program == crate::firewall_health::CURL {
            return self.run_host(program, args);
        }
        let mut command = Command::new(program);
        command.args(args);
        crate::firewall_process::run(
            command,
            std::time::Duration::from_secs(5),
            MAX_RULESET_BYTES,
        )
    }

    fn exists(&mut self, path: &Path) -> bool {
        path.is_file()
    }

    fn read_file(&mut self, path: &Path) -> Result<String, String> {
        std::fs::read_to_string(path).map_err(|error| error.to_string())
    }

    fn write_file(&mut self, path: &Path, text: &str) -> Result<(), String> {
        if text.len() > MAX_RULESET_BYTES {
            return Err("state file exceeds bound".into());
        }
        let parent = path
            .parent()
            .ok_or_else(|| "the state file has no parent directory".to_string())?;
        for ancestor in parent.ancestors() {
            match std::fs::symlink_metadata(ancestor) {
                Ok(metadata) if !metadata.is_dir() => return Err("invalid state directory".into()),
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|error| error.to_string())?;
        let metadata = std::fs::symlink_metadata(parent).map_err(|error| error.to_string())?;
        if !metadata.is_dir() || metadata.permissions().mode() & 0o022 != 0 {
            return Err("state directory is writable by other users".into());
        }
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if !metadata.is_file() => return Err("invalid state file".into()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        static SEQUENCE: AtomicU64 = AtomicU64::new(1);
        let temporary = parent.join(format!(
            ".firewall-{}-{}.tmp",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        let result = (|| {
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&temporary, path)?;
            std::fs::File::open(parent)?.sync_all()
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.map_err(|error: std::io::Error| error.to_string())
    }

    fn remove_file(&mut self, path: &Path) -> Result<(), String> {
        std::fs::remove_file(path).map_err(|error| error.to_string())
    }
}

pub struct FirewallState {
    mode_runtime: Option<crate::firewall_runtime::SharedModeRuntime>,
    runner: Box<dyn FirewallRunner>,
    state_dir: PathBuf,
    capability_enabled: bool,
    active: bool,
}

impl FirewallState {
    pub fn open(
        runner: Box<dyn FirewallRunner>,
        state_dir: impl Into<PathBuf>,
        capability_enabled: bool,
    ) -> Self {
        Self {
            runner,
            mode_runtime: None,
            state_dir: state_dir.into(),
            capability_enabled,
            active: false,
        }
    }

    pub fn attach_mode_runtime(&mut self, runtime: crate::firewall_runtime::SharedModeRuntime) {
        self.mode_runtime = Some(runtime);
    }

    pub fn mode_runtime(&self) -> Option<crate::firewall_runtime::SharedModeRuntime> {
        self.mode_runtime.clone()
    }

    /// Probes the reviewed backends and reports the baseline state. A missing
    /// backend keeps the surface unavailable rather than absent.
    pub fn view(&mut self) -> FirewallView {
        let nft = self.probe(NFT, &["--version"]);
        let iptables = self.probe(IPTABLES, &["--version"]);
        let ip6tables = self.probe(IP6TABLES, &["--version"]);
        let observation = if nft.installed {
            self.owned_handle()
        } else {
            Ok(None)
        };
        self.active = matches!(observation, Ok(Some(_)));
        let available = nft.installed && observation.is_ok();
        FirewallView {
            backend_available: available,
            baseline_active: self.active,
            mode: if observation.is_err() {
                "unknown"
            } else if self.active {
                "baseline"
            } else {
                "disabled"
            }
            .to_string(),
            https_port: 443,
            message: if observation.is_err() {
                "firewall ownership could not be verified; writes are refused".to_string()
            } else if available {
                "the reviewed nftables baseline is available".to_string()
            } else {
                "no reviewed firewall backend is installed; install nftables to \
                 enable the baseline"
                    .to_string()
            },
            nft,
            iptables,
            ip6tables,
            rules_ipv4: self.rules("ipv4"),
            rules_ipv6: self.rules("ipv6"),
            rules_nft: self.rules("nft"),
        }
    }

    /// Enables or disables the reviewed baseline. Only the baseline mode is
    /// offered; the reference's other modes would need rules this arrangement
    /// has not reviewed.
    pub fn set_baseline(&mut self, enabled: bool) -> ControlResponse {
        if self.mode_runtime.is_some() {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        if !self.capability_enabled || !self.probe(NFT, &["--version"]).installed {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        let handle = match self.owned_handle() {
            Ok(handle) => handle,
            Err(_) => return reject(ControlRejectionCode::BackendRejected),
        };
        self.active = handle.is_some();
        if enabled == self.active {
            return ControlResponse::Applied;
        }
        if enabled {
            let previous = match self.snapshot() {
                Ok(previous) => previous,
                Err(_) => return reject(ControlRejectionCode::BackendRejected),
            };
            // Retain the pre-activation observation for diagnosis/recovery. We
            // exclusively create one table, so rollback must not replay others.
            if self
                .runner
                .write_file(&self.state_dir.join("original.nft"), &previous)
                .is_err()
            {
                return reject(ControlRejectionCode::BackendRejected);
            }
            if let Err(reason) = self.apply_ruleset() {
                eprintln!("firewall: baseline apply failed: {reason}");
                return reject(ControlRejectionCode::BackendRejected);
            }
        } else if let Some(handle) = handle {
            if self
                .runner
                .run(
                    NFT,
                    &["delete", "table", "inet", "handle", &handle.to_string()],
                )
                .is_err()
            {
                return reject(ControlRejectionCode::BackendRejected);
            }
        }
        match self.owned_handle() {
            Ok(observed) if observed.is_some() == enabled => self.active = enabled,
            _ => return reject(ControlRejectionCode::BackendRejected),
        }
        ControlResponse::Applied
    }

    /// Reconcile with kernel truth; never adopt an unmarked or conflicting table.
    fn owned_handle(&mut self) -> Result<Option<u64>, String> {
        let output = self.runner.run(NFT, &["-j", "list", "tables"])?;
        if output.len() > MAX_RULESET_BYTES {
            return Err("table inventory exceeds bound".into());
        }
        let value: serde_json::Value =
            serde_json::from_str(&output).map_err(|_| "invalid table inventory")?;
        let entries = value
            .get("nftables")
            .and_then(serde_json::Value::as_array)
            .ok_or("missing table inventory")?;
        let mut owned = None;
        for entry in entries {
            let Some(table) = entry.get("table") else {
                continue;
            };
            if table.get("family").and_then(serde_json::Value::as_str) != Some("inet")
                || table.get("name").and_then(serde_json::Value::as_str) != Some("hardened-blikvm")
            {
                continue;
            }
            let comment = match table.get("comment") {
                Some(comment) => comment.as_str().map(str::to_owned),
                // nft 1.0.2 omits table comments from JSON; read the listing.
                None => {
                    let listing = self
                        .runner
                        .run(NFT, &["list", "table", "inet", "hardened-blikvm"])?;
                    crate::firewall_observation::text_table_comment(&listing).map(str::to_owned)
                }
            };
            if owned.is_some() || comment.as_deref() != Some(OWNER_COMMENT) {
                return Err("conflicting firewall table".into());
            }
            owned = Some(
                table
                    .get("handle")
                    .and_then(serde_json::Value::as_u64)
                    .filter(|handle| *handle > 0)
                    .ok_or("invalid table handle")?,
            );
        }
        Ok(owned)
    }

    fn probe(&mut self, program: &str, _args: &[&str]) -> BackendView {
        let path = Path::new(program);
        if self.runner.exists(path) {
            BackendView {
                installed: true,
                detail: program.to_string(),
            }
        } else {
            BackendView {
                installed: false,
                detail: format!("{program} is not installed"),
            }
        }
    }

    fn rules(&mut self, _family: &str) -> String {
        if !self.probe(NFT, &["--version"]).installed {
            return String::new();
        }
        self.runner
            .run(NFT, &["list", "ruleset"])
            .map(|mut output| {
                let mut end = MAX_RULESET_BYTES.min(output.len());
                while !output.is_char_boundary(end) {
                    end -= 1;
                }
                output.truncate(end);
                output
            })
            .unwrap_or_default()
    }

    fn snapshot(&mut self) -> Result<String, String> {
        let snapshot = self.runner.run(NFT, &["list", "ruleset"])?;
        if snapshot.len() > MAX_RULESET_BYTES {
            return Err("the rollback snapshot exceeds the reviewed bound".into());
        }
        Ok(snapshot)
    }

    /// The reviewed baseline: drop nothing, but allow established traffic and
    /// the management port, and drop inbound traffic that is not the front.
    fn apply_ruleset(&mut self) -> Result<(), String> {
        let ruleset = format!(
            "create table inet hardened-blikvm {{ comment \"{OWNER_COMMENT}\"; }}\ntable inet hardened-blikvm {{\n  chain input {{\n    type filter hook input priority 0; policy drop;\n    ct state established,related accept\n    iif lo accept\n    tcp dport {{ 22, {} }} accept\n  }}\n}}\n",
            self.https_port()
        );
        if ruleset.len() > MAX_RULESET_BYTES {
            return Err("the ruleset exceeds the reviewed bound".into());
        }
        let path = self.state_dir.join("baseline.nft");
        self.runner.write_file(&path, &ruleset)?;
        let result = self.runner.run(NFT, &["-f", &path.to_string_lossy()]);
        let _ = self.runner.remove_file(&path);
        result.map(|_| ())
    }

    fn https_port(&self) -> u16 {
        443
    }
}

fn reject(code: ControlRejectionCode) -> ControlResponse {
    ControlResponse::Rejected { code }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingRunner {
        installed: bool,
        fail_apply: bool,
        fail_snapshot: bool,
        ruleset: String,
        managed_present: bool,
        foreign_table: bool,
        calls: Arc<Mutex<Vec<String>>>,
        files: Arc<Mutex<HashMap<String, String>>>,
    }

    impl FirewallRunner for RecordingRunner {
        fn exists(&mut self, _path: &Path) -> bool {
            self.installed
        }

        fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
            let call = format!("{program} {}", args.join(" "));
            self.calls.lock().unwrap().push(call.clone());
            if args == ["-j", "list", "tables"] {
                return Ok(serde_json::json!({"nftables": if self.managed_present || self.foreign_table {
                    vec![serde_json::json!({"table": {"family": "inet", "name": "hardened-blikvm", "handle": 42,
                        "comment": if self.foreign_table { "foreign" } else { OWNER_COMMENT }}})]
                } else { vec![] }}).to_string());
            }
            if args.first() == Some(&"--version") {
                return if self.installed {
                    Ok("nftables v1.0.6".into())
                } else {
                    Err("not found".into())
                };
            }
            if self.fail_apply && call.contains("-f ") {
                return Err("apply failure".into());
            }
            if args.first() == Some(&"-f") {
                let text = self
                    .files
                    .lock()
                    .unwrap()
                    .get(args[1])
                    .cloned()
                    .ok_or_else(|| "missing input file".to_string())?;
                // nft -f adds declarations; replaying a snapshot does not remove
                // a table that is absent from that snapshot.
                if text.contains("table inet hardened-blikvm") {
                    self.managed_present = true;
                }
            }
            if args == ["delete", "table", "inet", "handle", "42"] {
                self.managed_present = false;
            }
            if call.contains("list ruleset") {
                return if self.fail_snapshot {
                    Err("snapshot unavailable".into())
                } else {
                    Ok(format!(
                        "{}{}",
                        self.ruleset,
                        if self.managed_present {
                            "table inet hardened-blikvm {}\n"
                        } else {
                            ""
                        }
                    ))
                };
            }
            Ok(String::new())
        }

        fn read_file(&mut self, _path: &Path) -> Result<String, String> {
            Ok(String::new())
        }

        fn write_file(&mut self, path: &Path, text: &str) -> Result<(), String> {
            self.files
                .lock()
                .unwrap()
                .insert(path.to_string_lossy().into_owned(), text.to_string());
            Ok(())
        }

        fn remove_file(&mut self, _path: &Path) -> Result<(), String> {
            Ok(())
        }
    }

    type Calls = Arc<Mutex<Vec<String>>>;
    type Files = Arc<Mutex<HashMap<String, String>>>;

    fn fixture(installed: bool, fail_apply: bool) -> (FirewallState, Calls, Files) {
        let runner = RecordingRunner {
            installed,
            fail_apply,
            ..Default::default()
        };
        let calls = runner.calls.clone();
        let files = runner.files.clone();
        (
            FirewallState::open(Box::new(runner), "/run/fw", true),
            calls,
            files,
        )
    }

    #[test]
    fn without_a_backend_the_surface_is_unavailable() {
        let (mut state, calls, _) = fixture(false, false);
        let view = state.view();
        assert!(!view.backend_available);
        assert_eq!(view.mode, "disabled");
        assert!(!view.nft.installed);
        assert!(view.message.contains("nftables"));
        // The mode request is refused and never applies anything.
        assert_eq!(
            state.set_baseline(true),
            ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled
            }
        );
        assert!(
            calls
                .lock()
                .unwrap()
                .iter()
                .all(|call| !call.contains("-f "))
        );
    }

    #[test]
    fn the_baseline_applies_exclusively_and_reports_apply_failure() {
        let (mut state, calls, files) = fixture(true, false);
        let view = state.view();
        assert!(view.backend_available);
        assert_eq!(state.set_baseline(true), ControlResponse::Applied);
        let written = files.lock().unwrap().clone();
        let ruleset = &written["/run/fw/baseline.nft"];
        assert!(ruleset.contains("create table inet hardened-blikvm"));
        assert!(ruleset.contains(OWNER_COMMENT));
        assert!(ruleset.contains("hardened-blikvm"));
        assert!(ruleset.contains("policy drop"));
        assert!(ruleset.contains("tcp dport { 22, 443 } accept"));
        assert!(
            calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call.contains("-f "))
        );
        assert_eq!(state.set_baseline(true), ControlResponse::Applied);
        assert_eq!(state.set_baseline(false), ControlResponse::Applied);

        // A failed atomic batch is refused, never replayed over unrelated tables.
        let (mut failing, calls, _) = fixture(true, true);
        assert_eq!(
            failing.set_baseline(true),
            ControlResponse::Rejected {
                code: ControlRejectionCode::BackendRejected
            }
        );
        assert!(
            calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call.contains("-f "))
        );
    }

    #[test]
    fn disabling_removes_managed_table_without_replaying_unrelated_rules() {
        let original = "table inet unrelated { chain input { tcp dport 8443 accept } }\n";
        let runner = RecordingRunner {
            installed: true,
            ruleset: original.into(),
            ..Default::default()
        };
        let calls = runner.calls.clone();
        let mut state = FirewallState::open(Box::new(runner), "/run/fw", true);
        assert_eq!(state.set_baseline(true), ControlResponse::Applied);
        assert!(
            state
                .view()
                .rules_nft
                .contains("table inet hardened-blikvm")
        );
        calls.lock().unwrap().clear();
        assert_eq!(state.set_baseline(false), ControlResponse::Applied);
        assert_eq!(state.view().rules_nft, original);
        assert!(
            !calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call.contains("-f ")),
            "disabling a newly owned table must not replay unrelated tables"
        );
    }

    #[test]
    fn restart_reconciles_owned_table_and_enable_is_idempotent() {
        let runner = RecordingRunner {
            installed: true,
            managed_present: true,
            ..Default::default()
        };
        let calls = runner.calls.clone();
        let files = runner.files.clone();
        let mut state = FirewallState::open(Box::new(runner), "/run/fw", true);
        assert!(state.view().baseline_active);
        assert_eq!(state.set_baseline(true), ControlResponse::Applied);
        assert!(files.lock().unwrap().is_empty());
        assert!(
            !calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call.contains("-f "))
        );
        assert_eq!(state.set_baseline(false), ControlResponse::Applied);
        assert!(!state.view().baseline_active);
        assert_eq!(state.set_baseline(false), ControlResponse::Applied);
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| call.contains("delete table"))
                .count(),
            1
        );
    }

    #[test]
    fn conflicting_table_is_not_adopted_or_deleted() {
        let runner = RecordingRunner {
            installed: true,
            foreign_table: true,
            ..Default::default()
        };
        let calls = runner.calls.clone();
        let files = runner.files.clone();
        let mut state = FirewallState::open(Box::new(runner), "/run/fw", true);
        let view = state.view();
        assert!(!view.backend_available);
        assert_eq!(view.mode, "unknown");
        for enabled in [false, true] {
            assert_eq!(
                state.set_baseline(enabled),
                reject(ControlRejectionCode::BackendRejected)
            );
        }
        assert!(files.lock().unwrap().is_empty());
        assert!(
            !calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call.contains("-f ") || call.contains("delete "))
        );
    }

    #[test]
    fn rules_output_truncation_preserves_utf8_boundaries() {
        let prefix = "x".repeat(MAX_RULESET_BYTES - 1);
        let runner = RecordingRunner {
            installed: true,
            ruleset: format!("{prefix}é"),
            ..Default::default()
        };
        let mut state = FirewallState::open(Box::new(runner), "/run/fw", true);
        let view = state.view();
        assert_eq!(view.rules_nft, prefix);
        assert!(view.rules_nft.len() <= MAX_RULESET_BYTES);
    }

    #[test]
    fn unreadable_or_oversized_snapshot_prevents_every_write() {
        for (fail_snapshot, ruleset) in [
            (true, String::new()),
            (false, "x".repeat(MAX_RULESET_BYTES + 1)),
        ] {
            let runner = RecordingRunner {
                installed: true,
                fail_snapshot,
                ruleset,
                ..Default::default()
            };
            let calls = runner.calls.clone();
            let files = runner.files.clone();
            let mut state = FirewallState::open(Box::new(runner), "/run/fw", true);
            assert_eq!(
                state.set_baseline(true),
                reject(ControlRejectionCode::BackendRejected)
            );
            assert!(files.lock().unwrap().is_empty());
            assert_eq!(
                *calls.lock().unwrap(),
                vec![
                    format!("{NFT} -j list tables"),
                    format!("{NFT} list ruleset")
                ]
            );
        }
    }

    #[test]
    fn state_files_are_private_atomically_replaced_and_reject_links() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state/original.nft");
        let mut runner = SystemFirewallRunner;
        runner.write_file(&path, "original").unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        runner.write_file(&path, "replacement").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "replacement");
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
        let link = directory.path().join("state/link.nft");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(runner.write_file(&link, "bad").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "replacement");
        let alias = directory.path().join("alias");
        std::os::unix::fs::symlink(path.parent().unwrap(), &alias).unwrap();
        assert!(runner.write_file(&alias.join("other.nft"), "bad").is_err());
        std::fs::set_permissions(
            path.parent().unwrap(),
            std::fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        assert!(runner.write_file(&path, "bad").is_err());
    }

    #[test]
    fn a_disabled_capability_never_touches_the_backend() {
        let runner = RecordingRunner {
            installed: true,
            ..Default::default()
        };
        let calls = runner.calls.clone();
        let mut state = FirewallState::open(Box::new(runner), "/run/fw", false);
        assert_eq!(
            state.set_baseline(true),
            ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled
            }
        );
        assert!(calls.lock().unwrap().is_empty());
    }
}
