//! Execute only in an explicitly verified disposable network namespace.
use bli_kvm_control_broker::{
    firewall_confirmation::PendingFileStore,
    firewall_executor::{ModeConfiguration, NftModeExecutor},
    firewall_rules::FirewallMode,
    firewall_settings::{FirewallRunner, NFT, SystemFirewallRunner},
    firewall_transaction::FirewallTransaction,
};
use std::{path::Path, process::Command};

struct DirectRunner(String);
impl FirewallRunner for DirectRunner {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<String, String> {
        // This namespace gate checks nft/transaction behavior; TLS validation
        // has its own gate and is explicitly simulated here.
        if program == bli_kvm_control_broker::firewall_health::CURL {
            return Ok(concat!(r#"{"code":0,"data":{"status":"ok","backend":"rust","platformId":"bli-h616-pikvm"}}"#, "\n200").into());
        }
        if program != NFT {
            return Err("unexpected executable".into());
        }
        let output = Command::new(&self.0)
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
        String::from_utf8(output.stdout).map_err(|e| e.to_string())
    }
    fn exists(&mut self, path: &Path) -> bool {
        path == Path::new(NFT) && Path::new(&self.0).is_file()
    }
    fn read_file(&mut self, path: &Path) -> Result<String, String> {
        SystemFirewallRunner.read_file(path)
    }
    fn write_file(&mut self, path: &Path, text: &str) -> Result<(), String> {
        SystemFirewallRunner.write_file(path, text)
    }
    fn remove_file(&mut self, path: &Path) -> Result<(), String> {
        SystemFirewallRunner.remove_file(path)
    }
}

fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("expected reviewed nft executable and parent net namespace".into());
    }
    let namespace = std::fs::read_link("/proc/self/ns/net").map_err(|e| e.to_string())?;
    if namespace.to_string_lossy() == args[2] {
        return Err("refusing parent network namespace".into());
    }
    let root = tempfile::tempdir().map_err(|e| e.to_string())?;
    let config = ModeConfiguration {
        https_probe: Some(bli_kvm_control_broker::firewall_health::HttpsProbe {
            server_name: "fixture.local".into(),
            ca_certificate: "/fixture/ca.crt".into(),
        }),
        https_enabled: true,
        https_port: 8443,
        syslog_port: 1514,
        backend_isolation: None,
    };
    let executor = NftModeExecutor::open_frozen(
        Box::new(DirectRunner(args[1].clone())),
        root.path().into(),
        config.clone(),
    )?;
    let mut tx = FirewallTransaction::open(PendingFileStore::new(root.path())?, executor, [3; 16])
        .map_err(|e| format!("{e:?}"))?;
    tx.request([1; 16], [2; 32], FirewallMode::Restricted, 100)
        .map_err(|e| format!("{e:?}"))?;
    assert_eq!(tx.current_mode().unwrap(), FirewallMode::Restricted);
    tx.tick(60100).map_err(|e| format!("{e:?}"))?;
    assert_eq!(tx.current_mode().unwrap(), FirewallMode::Baseline);
    tx.request([4; 16], [2; 32], FirewallMode::Paranoid, 70000)
        .map_err(|e| format!("{e:?}"))?;
    tx.confirm([4; 16], [2; 32], 70001)
        .map_err(|e| format!("{e:?}"))?;
    tx.tick(150000).unwrap();
    assert_eq!(tx.current_mode().unwrap(), FirewallMode::Paranoid);
    tx.request([5; 16], [2; 32], FirewallMode::Restricted, 160000)
        .map_err(|e| format!("{e:?}"))?;
    drop(tx);
    let mut changed_config = config.clone();
    changed_config.https_port = 9443;
    let executor = NftModeExecutor::open_frozen(
        Box::new(DirectRunner(args[1].clone())),
        root.path().into(),
        changed_config.clone(),
    )?;
    assert!(!executor.configuration_matches(&changed_config));
    assert!(executor.configuration_matches(&config));
    let mut tx = FirewallTransaction::open(PendingFileStore::new(root.path())?, executor, [3; 16])
        .map_err(|e| format!("{e:?}"))?;
    tx.tick(160001).map_err(|e| format!("{e:?}"))?;
    assert_eq!(tx.current_mode().unwrap(), FirewallMode::Paranoid);
    assert!(tx.pending().is_none());
    assert_eq!(tx.request([6;16], [2;32], FirewallMode::Baseline, 170000), Err(bli_kvm_control_broker::firewall_transaction::TransactionError::ConfigurationUnavailable));
    drop(tx);
    let executor = NftModeExecutor::open_frozen(
        Box::new(DirectRunner(args[1].clone())),
        root.path().into(),
        config.clone(),
    )?;
    let mut tx = FirewallTransaction::open(PendingFileStore::new(root.path())?, executor, [3; 16])
        .map_err(|e| format!("{e:?}"))?;
    tx.request([7; 16], [2; 32], FirewallMode::Restricted, 180000)
        .map_err(|e| format!("{e:?}"))?;
    drop(tx);
    let executor = NftModeExecutor::reopen_for_recovery(
        Box::new(DirectRunner(args[1].clone())),
        root.path().into(),
    )?;
    let mut tx = FirewallTransaction::open(PendingFileStore::new(root.path())?, executor, [3; 16])
        .map_err(|e| format!("{e:?}"))?;
    tx.tick(180001).map_err(|e| format!("{e:?}"))?;
    assert_eq!(tx.current_mode().unwrap(), FirewallMode::Paranoid);
    assert_eq!(tx.request([8;16],[2;32],FirewallMode::Baseline,180002),Err(bli_kvm_control_broker::firewall_transaction::TransactionError::ConfigurationUnavailable));
    drop(tx);
    let executor = NftModeExecutor::open_frozen(
        Box::new(DirectRunner(args[1].clone())),
        root.path().into(),
        config,
    )?;
    let mut tx = FirewallTransaction::open(PendingFileStore::new(root.path())?, executor, [3; 16])
        .map_err(|e| format!("{e:?}"))?;
    DirectRunner(args[1].clone()).run(
        NFT,
        &[
            "add",
            "chain",
            "inet",
            "hardened-blikvm",
            "input",
            "{ policy accept; }",
        ],
    )?;
    assert!(
        tx.current_mode().is_err(),
        "modified chain policy was accepted"
    );
    assert!(
        tx.request([6; 16], [2; 32], FirewallMode::Baseline, 170000)
            .is_err(),
        "unverified rules were overwritten"
    );
    println!(
        "nft executor: apply, semantic read-back, timeout rollback, confirmation and process restart recovery passed"
    );
    Ok(())
}
