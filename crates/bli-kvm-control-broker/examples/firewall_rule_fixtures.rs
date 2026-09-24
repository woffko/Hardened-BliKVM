//! Emit generated policies for the isolated nft parser/runtime gate.
use bli_kvm_control_broker::firewall_rules::{FirewallMode, FirewallPorts, render};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let verify = std::env::args().nth(2).as_deref() == Some("--verify");
    let directory = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("fixture directory required")?,
    );
    if !directory.is_dir() {
        return Err("fixture directory must already exist".into());
    }
    let ports = FirewallPorts {
        https_enabled: true,
        https_port: 8443,
        syslog_port: 1514,
        backend_isolation: None,
    };
    for mode in [
        FirewallMode::Baseline,
        FirewallMode::Moderate,
        FirewallMode::Restricted,
        FirewallMode::Paranoid,
    ] {
        if verify {
            let observed =
                std::fs::read_to_string(directory.join(format!("{}.json", mode.as_str())))?;
            bli_kvm_control_broker::firewall_observation::verify(&observed, mode, &ports)?;
            let mut altered: serde_json::Value = serde_json::from_str(&observed)?;
            let rule = altered["nftables"]
                .as_array_mut()
                .ok_or("missing nftables")?
                .iter_mut()
                .find(|entry| entry.get("rule").is_some())
                .ok_or("missing rule")?;
            rule["rule"]["expr"] = serde_json::json!([{"accept": null}]);
            if bli_kvm_control_broker::firewall_observation::verify(
                &altered.to_string(),
                mode,
                &ports,
            )
            .is_ok()
            {
                return Err("overly permissive rule accepted".into());
            }
            continue;
        }
        std::fs::write(
            directory.join(format!("{}.nft", mode.as_str())),
            render(mode, &ports)?,
        )?;
    }
    Ok(())
}
