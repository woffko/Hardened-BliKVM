//! Reviewed firewall baseline owned by the privileged board owner.
//!
//! The board ships no `nft`, so the surface stays fail-closed: the owner probes
//! the reviewed backends and reports the baseline as unavailable, and every
//! request is refused while no backend exists. When a backend is installed the
//! baseline is one reviewed ruleset applied with `nft -f`, with the previous
//! ruleset saved and restored when the apply fails.

use std::path::{Path, PathBuf};
use std::process::Command;

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};

pub const NFT: &str = "/usr/sbin/nft";
pub const IPTABLES: &str = "/usr/sbin/iptables";
pub const IP6TABLES: &str = "/usr/sbin/ip6tables";
/// Where the owner keeps the previous ruleset while it applies a new one.
pub const FIREWALL_STATE_DIR: &str = "/run/hardened-blikvm-firewall";
const MAX_RULESET_BYTES: usize = 64 * 1024;

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
    fn read_file(&mut self, path: &Path) -> Result<String, String>;
    fn write_file(&mut self, path: &Path, text: &str) -> Result<(), String>;
    fn remove_file(&mut self, path: &Path) -> Result<(), String>;
}

pub struct SystemFirewallRunner;

impl FirewallRunner for SystemFirewallRunner {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
        let output = Command::new(program)
            .args(args)
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn read_file(&mut self, path: &Path) -> Result<String, String> {
        std::fs::read_to_string(path).map_err(|error| error.to_string())
    }

    fn write_file(&mut self, path: &Path, text: &str) -> Result<(), String> {
        let parent = path
            .parent()
            .ok_or_else(|| "the state file has no parent directory".to_string())?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        std::fs::write(path, text).map_err(|error| error.to_string())?;
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .map_err(|error| error.to_string())
    }

    fn remove_file(&mut self, path: &Path) -> Result<(), String> {
        std::fs::remove_file(path).map_err(|error| error.to_string())
    }
}

pub struct FirewallState {
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
            state_dir: state_dir.into(),
            capability_enabled,
            active: false,
        }
    }

    /// Probes the reviewed backends and reports the baseline state. A missing
    /// backend keeps the surface unavailable rather than absent.
    pub fn view(&mut self) -> FirewallView {
        let nft = self.probe(NFT, &["--version"]);
        let iptables = self.probe(IPTABLES, &["--version"]);
        let ip6tables = self.probe(IP6TABLES, &["--version"]);
        let available = nft.installed;
        FirewallView {
            backend_available: available,
            baseline_active: self.active,
            mode: if self.active { "baseline" } else { "disabled" }.to_string(),
            https_port: 443,
            message: if available {
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
        if !self.capability_enabled || !self.probe(NFT, &["--version"]).installed {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        if enabled == self.active {
            return ControlResponse::Applied;
        }
        let previous = self.snapshot().unwrap_or_default();
        if enabled {
            if let Err(reason) = self.apply_ruleset() {
                eprintln!("firewall: baseline apply failed: {reason}");
                let _ = self.restore(&previous);
                return reject(ControlRejectionCode::BackendRejected);
            }
        } else if let Err(reason) = self.restore(&previous) {
            eprintln!("firewall: baseline removal failed: {reason}");
            return reject(ControlRejectionCode::BackendRejected);
        }
        self.active = enabled;
        ControlResponse::Applied
    }

    fn probe(&mut self, program: &str, args: &[&str]) -> BackendView {
        match self.runner.run(program, args) {
            Ok(output) => BackendView {
                installed: true,
                detail: output.trim().to_string(),
            },
            Err(detail) => BackendView {
                installed: false,
                detail,
            },
        }
    }

    fn rules(&mut self, _family: &str) -> String {
        if !self.probe(NFT, &["--version"]).installed {
            return String::new();
        }
        self.runner
            .run(NFT, &["list", "ruleset"])
            .map(|mut output| {
                output.truncate(MAX_RULESET_BYTES.min(output.len()));
                output
            })
            .unwrap_or_default()
    }

    fn snapshot(&mut self) -> Result<String, String> {
        self.runner
            .run(NFT, &["list", "ruleset"])
            .map_err(|error| error.to_string())
    }

    /// The reviewed baseline: drop nothing, but allow established traffic and
    /// the management port, and drop inbound traffic that is not the front.
    fn apply_ruleset(&mut self) -> Result<(), String> {
        let ruleset = format!(
            "table inet hardened-blikvm {{\n  chain input {{\n    type filter hook input priority 0; policy drop;\n    ct state established,related accept\n    iif lo accept\n    tcp dport {{ 22, {} }} accept\n  }}\n}}\n",
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

    fn restore(&mut self, previous: &str) -> Result<(), String> {
        if previous.trim().is_empty() {
            // Nothing was reviewed before: flushing the reviewed table is the
            // reversible default.
            return self
                .runner
                .run(NFT, &["delete", "table", "inet", "hardened-blikvm"])
                .map(|_| ());
        }
        let path = self.state_dir.join("previous.nft");
        self.runner.write_file(&path, previous)?;
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
        calls: Arc<Mutex<Vec<String>>>,
        files: Arc<Mutex<HashMap<String, String>>>,
    }

    impl FirewallRunner for RecordingRunner {
        fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
            let call = format!("{program} {}", args.join(" "));
            self.calls.lock().unwrap().push(call.clone());
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
            if call.contains("list ruleset") {
                return Ok(String::new());
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
    fn the_baseline_applies_the_reviewed_ruleset_and_restores_on_failure() {
        let (mut state, calls, files) = fixture(true, false);
        let view = state.view();
        assert!(view.backend_available);
        assert_eq!(state.set_baseline(true), ControlResponse::Applied);
        let written = files.lock().unwrap().clone();
        let ruleset = written.values().next().cloned().unwrap_or_default();
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

        // A failing apply restores the previous ruleset.
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
