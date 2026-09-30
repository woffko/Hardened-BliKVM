//! Emit generated policies for the isolated nft parser/runtime gate.
use bli_kvm_control_broker::firewall_rules::{FirewallMode, FirewallPorts, render};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let verify = std::env::args().nth(2).as_deref() == Some("--verify");
    // Usage: firewall_rule_fixtures DIR [--verify|--render] [WEBRTC_RANGE]
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
        // Optional third argument: an inclusive WebRTC media range such as
        // 20000-20099, to exercise that allowance against the real nft.
        webrtc_media: match std::env::args().nth(3) {
            None => None,
            Some(range) => {
                let (first, last) = range.split_once('-').ok_or("range must be FIRST-LAST")?;
                Some((first.parse()?, last.parse()?))
            }
        },
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
            // nft 1.0.2 omits the table comment from JSON; the mode then comes
            // from the fixture name, as the executor reads it from the listing.
            let source = if observed.contains("\"comment\"") {
                bli_kvm_control_broker::firewall_observation::CommentSource::Json
            } else {
                bli_kvm_control_broker::firewall_observation::CommentSource::Text
            };
            bli_kvm_control_broker::firewall_observation::verify_with(
                &observed, mode, &ports, source,
            )?;
            let mut altered: serde_json::Value = serde_json::from_str(&observed)?;
            let rule = altered["nftables"]
                .as_array_mut()
                .ok_or("missing nftables")?
                .iter_mut()
                .find(|entry| entry.get("rule").is_some())
                .ok_or("missing rule")?;
            rule["rule"]["expr"] = serde_json::json!([{"accept": null}]);
            if bli_kvm_control_broker::firewall_observation::verify_with(
                &altered.to_string(),
                mode,
                &ports,
                source,
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
