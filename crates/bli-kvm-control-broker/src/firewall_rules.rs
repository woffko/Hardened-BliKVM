//! Pure native rules generation from the pinned reference policy matrix.
//! Not applied until the operation-bound confirmation/rollback path is wired.

pub use bli_kvm_core::FirewallMode;

/// How a mode's rules read in the kernel. The owner verifies its whole table
/// against the mode it is marked with, so it must also know what an earlier
/// build wrote for that mode: after an update it finds those rules in force
/// and brings them up to date (`firewall_executor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleForm {
    /// What `render` writes.
    Current,
    /// Restricted before it admitted the QUIC transport's port.
    BeforeQuicPort,
}

impl RuleForm {
    /// The forms a table of `mode` may have, this build's first.
    pub fn of(mode: FirewallMode) -> &'static [RuleForm] {
        if mode == FirewallMode::Restricted {
            &[RuleForm::Current, RuleForm::BeforeQuicPort]
        } else {
            &[RuleForm::Current]
        }
    }
}

/// Set only from a verified native front/backend topology, never a copied eth0.
pub struct BackendIsolation<'a> {
    pub interface: &'a str,
    pub source_port: u16,
}

pub struct FirewallPorts<'a> {
    pub https_enabled: bool,
    pub https_port: u16,
    pub syslog_port: u16,
    pub backend_isolation: Option<BackendIsolation<'a>>,
    /// Inclusive UDP range for WebRTC media (Janus), accepted only from local
    /// networks in the restrictive modes; `None` keeps the reference policy.
    pub webrtc_media: Option<(u16, u16)>,
}

const LOCAL_V4: &str = "{ 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 169.254.0.0/16 }";
const LOCAL_V6: &str = "{ fc00::/7, fe80::/10 }";
const IPV6_CONTROL: &str = "meta l4proto ipv6-icmp icmpv6 type { destination-unreachable, packet-too-big, time-exceeded, parameter-problem, nd-router-solicit, nd-router-advert, nd-neighbor-solicit, nd-neighbor-advert } accept\n";

pub fn render(mode: FirewallMode, ports: &FirewallPorts<'_>) -> Result<String, &'static str> {
    if ports.https_port == 0 || ports.syslog_port == 0 {
        return Err("invalid firewall port");
    }
    if mode.requires_confirmation() && !ports.https_enabled {
        return Err("HTTPS is required for restrictive firewall modes");
    }
    if ports
        .webrtc_media
        .is_some_and(|(first, last)| first < 1024 || first > last || last - first > 1000)
    {
        return Err("invalid WebRTC media range");
    }
    if let Some(isolation) = &ports.backend_isolation {
        if isolation.source_port == 0
            || isolation.interface.is_empty()
            || isolation.interface.len() > 15
            || !isolation
                .interface
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err("invalid backend isolation target");
        }
    }
    let restricted = mode.requires_confirmation();
    let baseline = mode == FirewallMode::Baseline;
    let mut input = String::from("iifname \"lo\" accept\nct state established,related accept\n");
    let mut output = String::from("oifname \"lo\" accept\n");
    if restricted {
        output.push_str("ct state established,related accept\n");
    }
    if !baseline {
        input.push_str("meta nfproto ipv4 udp sport 67 udp dport 68 accept\nmeta nfproto ipv6 udp sport 547 udp dport 546 accept\n");
        input.push_str(IPV6_CONTROL);
        output.push_str("meta nfproto ipv4 udp sport 68 udp dport 67 accept\nmeta nfproto ipv6 udp sport 546 udp dport 547 accept\n");
        output.push_str(IPV6_CONTROL);
        let accepted_ports = if mode == FirewallMode::Restricted {
            format!("tcp dport {{ 22, {} }} ", ports.https_port)
        } else if mode == FirewallMode::Paranoid {
            format!("tcp dport {} ", ports.https_port)
        } else {
            String::new()
        };
        input.push_str(&format!("ip saddr {LOCAL_V4} {accepted_ports}ct state new accept\nip6 saddr {LOCAL_V6} {accepted_ports}ct state new accept\n"));
        if mode == FirewallMode::Restricted {
            // The QUIC transport answers on the UDP side of the page's port
            // (docs/design/quic-transport-v1.md) and is admitted like the
            // page itself. Paranoid keeps it closed: HTTPS only.
            let port = ports.https_port;
            input.push_str(&format!("ip saddr {LOCAL_V4} udp dport {port} accept\nip6 saddr {LOCAL_V6} udp dport {port} accept\n"));
        }
        if restricted {
            if let Some((first, last)) = ports.webrtc_media {
                input.push_str(&format!("ip saddr {LOCAL_V4} udp dport {first}-{last} accept\nip6 saddr {LOCAL_V6} udp dport {first}-{last} accept\n"));
            }
        }
    }
    if mode == FirewallMode::Restricted {
        output.push_str(&format!(
            "tcp dport {{ 53, 443 }} ct state new accept\nudp dport {{ 53, 123, {} }} accept\n",
            ports.syslog_port
        ));
    }
    if !restricted {
        if let Some(isolation) = &ports.backend_isolation {
            output.push_str(&format!(
                "meta nfproto ipv4 oifname \"{}\" tcp sport {} ct state established drop\n",
                isolation.interface, isolation.source_port
            ));
        }
    }
    let input_policy = if baseline { "accept" } else { "drop" };
    let output_policy = if restricted { "drop" } else { "accept" };
    Ok(format!(
        "create table inet hardened-blikvm {{ comment \"hardened-blikvm-owner-v2:{}\"; }}\ntable inet hardened-blikvm {{\nchain input {{ type filter hook input priority 0; policy {input_policy};\n{input}}}\nchain forward {{ type filter hook forward priority 0; policy {input_policy}; }}\nchain output {{ type filter hook output priority 0; policy {output_policy};\n{output}}}\n}}\n",
        mode.as_str()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ports() -> FirewallPorts<'static> {
        FirewallPorts {
            https_enabled: true,
            https_port: 8443,
            syslog_port: 1514,
            backend_isolation: None,
            webrtc_media: None,
        }
    }

    #[test]
    fn webrtc_media_is_local_only_restrictive_only_and_bounded() {
        let mut config = ports();
        config.webrtc_media = Some((20000, 20099));
        for mode in [FirewallMode::Restricted, FirewallMode::Paranoid] {
            let rules = render(mode, &config).unwrap();
            assert!(rules.contains(&format!("ip saddr {LOCAL_V4} udp dport 20000-20099 accept")));
            assert!(rules.contains(&format!(
                "ip6 saddr {LOCAL_V6} udp dport 20000-20099 accept"
            )));
        }
        for mode in [FirewallMode::Baseline, FirewallMode::Moderate] {
            assert!(!render(mode, &config).unwrap().contains("20000-20099"));
        }
        for invalid in [(1000, 1100), (20100, 20000), (20000, 21001)] {
            config.webrtc_media = Some(invalid);
            assert!(render(FirewallMode::Restricted, &config).is_err());
        }
    }

    #[test]
    fn restrictive_modes_require_https_and_preserve_network_bootstrap() {
        for mode in [
            FirewallMode::Moderate,
            FirewallMode::Restricted,
            FirewallMode::Paranoid,
        ] {
            let rules = render(mode, &ports()).unwrap();
            for required in [
                "udp sport 67 udp dport 68",
                "udp sport 547 udp dport 546",
                "nd-neighbor-solicit",
                "packet-too-big",
                LOCAL_V4,
                LOCAL_V6,
            ] {
                assert!(rules.contains(required), "{mode:?}: {required}");
            }
            assert!(!rules.contains("flush ruleset"));
            assert!(!rules.contains("eth0"));
        }
        let mut config = ports();
        config.https_enabled = false;
        assert!(render(FirewallMode::Restricted, &config).is_err());
        assert!(render(FirewallMode::Paranoid, &config).is_err());
        assert!(render(FirewallMode::Moderate, &config).is_ok());
    }

    #[test]
    fn policy_differences_and_native_ports_are_explicit() {
        let config = ports();
        let restricted = render(FirewallMode::Restricted, &config).unwrap();
        let paranoid = render(FirewallMode::Paranoid, &config).unwrap();
        assert!(restricted.contains("tcp dport { 22, 8443 }"));
        assert!(restricted.contains("udp dport { 53, 123, 1514 }"));
        assert!(paranoid.contains("tcp dport 8443"));
        // The QUIC transport's port, the UDP side of the page's: Restricted
        // admits it from the local ranges, and no other mode names it.
        assert!(restricted.contains(&format!("ip saddr {LOCAL_V4} udp dport 8443 accept\n")));
        assert!(restricted.contains(&format!("ip6 saddr {LOCAL_V6} udp dport 8443 accept\n")));
        for mode in [
            FirewallMode::Baseline,
            FirewallMode::Moderate,
            FirewallMode::Paranoid,
        ] {
            assert!(
                !render(mode, &config).unwrap().contains("udp dport 8443"),
                "{mode:?}"
            );
        }
        assert_eq!(
            RuleForm::of(FirewallMode::Restricted),
            [RuleForm::Current, RuleForm::BeforeQuicPort]
        );
        assert_eq!(RuleForm::of(FirewallMode::Paranoid), [RuleForm::Current]);
        assert!(!paranoid.contains("dport { 53"));
        assert!(!paranoid.contains("22,"));
        assert!(
            !render(FirewallMode::Baseline, &config)
                .unwrap()
                .contains("policy drop")
        );
    }

    #[test]
    fn isolation_is_explicit_and_cannot_inject_rules() {
        let mut config = ports();
        config.backend_isolation = Some(BackendIsolation {
            interface: "enp1s0",
            source_port: 9000,
        });
        assert!(
            render(FirewallMode::Moderate, &config)
                .unwrap()
                .contains("oifname \"enp1s0\" tcp sport 9000")
        );
        config.backend_isolation.as_mut().unwrap().interface = "x\"; flush ruleset";
        assert!(render(FirewallMode::Baseline, &config).is_err());
        config.backend_isolation = None;
        config.https_port = 0;
        assert!(render(FirewallMode::Moderate, &config).is_err());
    }
}
