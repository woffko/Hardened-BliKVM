//! Verify the complete owned-table policy against observed nft JSON (schema 1).
use crate::firewall_rules::{FirewallMode, FirewallPorts, RuleForm, render};
use serde_json::{Value, json};

fn condition(left: Value, right: Value) -> Value {
    json!({"match":{"op":"==","left":left,"right":right}})
}
fn meta(key: &str, right: Value) -> Value {
    condition(json!({"meta":{"key":key}}), right)
}
fn payload(protocol: &str, field: &str, right: Value) -> Value {
    condition(
        json!({"payload":{"protocol":protocol,"field":field}}),
        right,
    )
}
fn state(right: Value) -> Value {
    json!({"match":{"op":"in","left":{"ct":{"key":"state"}},"right":right}})
}
fn rule(chain: &str, mut expr: Vec<Value>, verdict: &str) -> Value {
    expr.push(json!({verdict: null}));
    json!({"family":"inet", "table":"hardened-blikvm", "chain":chain, "expr":expr})
}
fn normalize(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(normalize),
        Value::Object(object) => {
            object.values_mut().for_each(normalize);
            if let Some(Value::Array(items)) = object.get_mut("set") {
                items.sort_by_key(Value::to_string);
                items.dedup();
            }
        }
        _ => {}
    }
}

fn v4_local() -> Value {
    json!({"set":[{"prefix":{"addr":"10.0.0.0","len":8}}, {"prefix":{"addr":"172.16.0.0","len":12}}, {"prefix":{"addr":"192.168.0.0","len":16}}, {"prefix":{"addr":"169.254.0.0","len":16}}]})
}
fn v6_local() -> Value {
    json!({"set":[{"prefix":{"addr":"fc00::","len":7}}, {"prefix":{"addr":"fe80::","len":10}}]})
}

fn expected_rules(mode: FirewallMode, ports: &FirewallPorts<'_>, form: RuleForm) -> Vec<Value> {
    let restricted = mode.requires_confirmation();
    let mut input = vec![
        rule("input", vec![meta("iifname", json!("lo"))], "accept"),
        rule(
            "input",
            vec![state(json!(["established", "related"]))],
            "accept",
        ),
    ];
    let mut output = vec![rule("output", vec![meta("oifname", json!("lo"))], "accept")];
    if restricted {
        output.push(rule(
            "output",
            vec![state(json!(["established", "related"]))],
            "accept",
        ));
    }
    if mode != FirewallMode::Baseline {
        for (chain, rules, outbound) in
            [("input", &mut input, false), ("output", &mut output, true)]
        {
            for (family, server, client) in [("ipv4", 67, 68), ("ipv6", 547, 546)] {
                let (source, dest) = if outbound {
                    (client, server)
                } else {
                    (server, client)
                };
                rules.push(rule(
                    chain,
                    vec![
                        meta("nfproto", json!(family)),
                        payload("udp", "sport", json!(source)),
                        payload("udp", "dport", json!(dest)),
                    ],
                    "accept",
                ));
            }
            rules.push(rule(chain, vec![payload("icmpv6", "type", json!({"set":["destination-unreachable", "packet-too-big", "time-exceeded", "parameter-problem", "nd-router-solicit", "nd-router-advert", "nd-neighbor-solicit", "nd-neighbor-advert"]}))], "accept"));
        }
        for (protocol, addresses) in [("ip", v4_local()), ("ip6", v6_local())] {
            let mut expr = vec![payload(protocol, "saddr", addresses)];
            if mode == FirewallMode::Restricted {
                expr.push(payload(
                    "tcp",
                    "dport",
                    json!({"set":[22,ports.https_port]}),
                ));
            }
            if mode == FirewallMode::Paranoid {
                expr.push(payload("tcp", "dport", json!(ports.https_port)));
            }
            expr.push(state(json!("new")));
            input.push(rule("input", expr, "accept"));
        }
        if mode == FirewallMode::Restricted && form == RuleForm::Current {
            for (protocol, addresses) in [("ip", v4_local()), ("ip6", v6_local())] {
                input.push(rule(
                    "input",
                    vec![
                        payload(protocol, "saddr", addresses),
                        payload("udp", "dport", json!(ports.https_port)),
                    ],
                    "accept",
                ));
            }
        }
        if restricted {
            if let Some((first, last)) = ports.webrtc_media {
                for (protocol, addresses) in [("ip", v4_local()), ("ip6", v6_local())] {
                    input.push(rule(
                        "input",
                        vec![
                            payload(protocol, "saddr", addresses),
                            payload("udp", "dport", json!({"range": [first, last]})),
                        ],
                        "accept",
                    ));
                }
            }
        }
    }
    if mode == FirewallMode::Restricted {
        output.push(rule(
            "output",
            vec![
                payload("tcp", "dport", json!({"set":[53,443]})),
                state(json!("new")),
            ],
            "accept",
        ));
        output.push(rule(
            "output",
            vec![payload(
                "udp",
                "dport",
                json!({"set":[53,123,ports.syslog_port]}),
            )],
            "accept",
        ));
    }
    if !restricted {
        if let Some(isolation) = &ports.backend_isolation {
            output.push(rule(
                "output",
                vec![
                    meta("nfproto", json!("ipv4")),
                    meta("oifname", json!(isolation.interface)),
                    payload("tcp", "sport", json!(isolation.source_port)),
                    state(json!("established")),
                ],
                "drop",
            ));
        }
    }
    input.extend(output);
    input
}

/// Where the owned table's mode marker was read. nft 1.0.2 (Ubuntu 22.04)
/// stores table comments but omits them from `-j` output; the marker is then
/// read from the text listing and the JSON table must carry no comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentSource {
    Json,
    Text,
}

/// Owned-table comment from `nft list table inet hardened-blikvm`, bounded
/// and exact: the header line must be followed directly by the comment.
pub fn text_table_comment(listing: &str) -> Option<&str> {
    if listing.len() > 64 * 1024 {
        return None;
    }
    let mut lines = listing.lines();
    if lines.next()? != "table inet hardened-blikvm {" {
        return None;
    }
    let comment = lines
        .next()?
        .trim()
        .strip_prefix("comment \"")?
        .strip_suffix('"')?;
    (!comment.contains('"') && !comment.contains('\\')).then_some(comment)
}

pub fn verify(
    document: &str,
    mode: FirewallMode,
    ports: &FirewallPorts<'_>,
) -> Result<u64, &'static str> {
    verify_with(document, mode, ports, CommentSource::Json)
}

pub fn verify_with(
    document: &str,
    mode: FirewallMode,
    ports: &FirewallPorts<'_>,
    source: CommentSource,
) -> Result<u64, &'static str> {
    verify_form(document, mode, ports, source, RuleForm::Current)
}

/// Verifies the owned table against the rules of `mode` in one of the forms
/// a build has written them in.
pub fn verify_form(
    document: &str,
    mode: FirewallMode,
    ports: &FirewallPorts<'_>,
    source: CommentSource,
    form: RuleForm,
) -> Result<u64, &'static str> {
    render(mode, ports)?;
    if document.len() > 64 * 1024 {
        return Err("oversized nft observation");
    }
    let data: Value = serde_json::from_str(document).map_err(|_| "invalid nft JSON")?;
    let entries = data
        .get("nftables")
        .and_then(Value::as_array)
        .ok_or("missing nft objects")?;
    let mut handle = None;
    let mut chains = Vec::new();
    let mut rules = Vec::new();
    for entry in entries {
        let object = entry.as_object().ok_or("invalid nft object")?;
        if object.len() != 1 {
            return Err("ambiguous nft object");
        }
        if let Some(meta) = object.get("metainfo") {
            if meta["json_schema_version"] != 1 {
                return Err("unsupported nft schema");
            }
            continue;
        }
        for (kind, body) in object {
            let mut body = body.as_object().cloned().ok_or("invalid nft body")?;
            let observed_handle = body
                .remove("handle")
                .and_then(|value| value.as_u64())
                .filter(|value| *value > 0)
                .ok_or("invalid nft handle")?;
            match kind.as_str() {
                "table" => {
                    let expected = match source {
                        CommentSource::Json => {
                            json!({"family":"inet","name":"hardened-blikvm","comment":format!("hardened-blikvm-owner-v2:{}",mode.as_str())})
                        }
                        CommentSource::Text => json!({"family":"inet","name":"hardened-blikvm"}),
                    };
                    if handle.is_some() || Value::Object(body) != expected {
                        return Err("wrong nft table");
                    }
                    handle = Some(observed_handle);
                }
                "chain" => chains.push(Value::Object(body)),
                "rule" => rules.push(Value::Object(body)),
                _ => return Err("unexpected nft object"),
            }
        }
    }
    let expected_chains: Vec<Value> = ["input","forward","output"].into_iter().map(|chain| json!({"family":"inet","table":"hardened-blikvm","name":chain,"type":"filter","hook":chain,"prio":0,"policy":if mode == FirewallMode::Baseline || (chain == "output" && !mode.requires_confirmation()) { "accept" } else { "drop" }})).collect();
    if chains != expected_chains {
        return Err("nft chain policy mismatch");
    }
    let mut actual = Value::Array(rules);
    let mut expected = Value::Array(expected_rules(mode, ports, form));
    normalize(&mut actual);
    normalize(&mut expected);
    if actual != expected {
        return Err("nft rule content mismatch");
    }
    handle.ok_or("missing owned table")
}

/// The owned table as `nft -j list table` prints it for `mode`, with the
/// rules in `form`: what a kernel holds after a build wrote them.
#[cfg(test)]
pub(crate) fn observed_document(
    mode: FirewallMode,
    ports: &FirewallPorts<'_>,
    form: RuleForm,
) -> String {
    let mut nftables = vec![
        json!({"metainfo": {"json_schema_version": 1}}),
        json!({"table": {"family": "inet", "name": "hardened-blikvm", "handle": 7, "comment": format!("hardened-blikvm-owner-v2:{}", mode.as_str())}}),
    ];
    for chain in ["input", "forward", "output"] {
        let policy = if mode == FirewallMode::Baseline
            || (chain == "output" && !mode.requires_confirmation())
        {
            "accept"
        } else {
            "drop"
        };
        nftables.push(json!({"chain": {"family": "inet", "table": "hardened-blikvm", "name": chain, "handle": 1, "type": "filter", "hook": chain, "prio": 0, "policy": policy}}));
    }
    for (index, mut rule) in expected_rules(mode, ports, form).into_iter().enumerate() {
        rule["handle"] = json!(index + 10);
        nftables.push(json!({ "rule": rule }));
    }
    json!({ "nftables": nftables }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed(mode: FirewallMode, ports: &FirewallPorts<'_>) -> String {
        observed_document(mode, ports, RuleForm::Current)
    }

    fn ports(webrtc_media: Option<(u16, u16)>) -> FirewallPorts<'static> {
        FirewallPorts {
            https_enabled: true,
            https_port: 8443,
            syslog_port: 1514,
            backend_isolation: None,
            webrtc_media,
        }
    }

    #[test]
    fn text_listing_comment_is_parsed_exactly() {
        let listing = "table inet hardened-blikvm {\n\tcomment \"hardened-blikvm-owner-v2:paranoid\"\n\tchain input {\n";
        assert_eq!(
            text_table_comment(listing),
            Some("hardened-blikvm-owner-v2:paranoid")
        );
        for invalid in [
            "table inet other {\n\tcomment \"x\"\n",
            "table inet hardened-blikvm {\n\tchain input {\n",
            "table inet hardened-blikvm {\n\tcomment \"a\"b\"\n",
            "table inet hardened-blikvm {\n\tcomment \"unterminated\n",
            "",
        ] {
            assert_eq!(text_table_comment(invalid), None, "{invalid:?}");
        }
    }

    #[test]
    fn nft_1_0_2_json_without_table_comment_needs_the_text_source() {
        // nft 1.0.2 omits the table comment from JSON; emulate that output.
        let config = ports(None);
        let mode = FirewallMode::Restricted;
        let mut document: Value = serde_json::from_str(&observed(mode, &config)).unwrap();
        document["nftables"][1]["table"]
            .as_object_mut()
            .unwrap()
            .remove("comment");
        let document = document.to_string();
        assert!(verify(&document, mode, &config).is_err());
        assert_eq!(
            verify_with(&document, mode, &config, CommentSource::Text),
            Ok(7)
        );
        // A JSON comment must not be present when the text source is used.
        assert!(verify_with(&observed(mode, &config), mode, &config, CommentSource::Text).is_err());
    }

    #[test]
    fn restricted_before_the_quic_port_is_another_form_of_the_same_mode() {
        let mode = FirewallMode::Restricted;
        for config in [ports(None), ports(Some((20000, 20099)))] {
            let current = observed(mode, &config);
            let earlier = observed_document(mode, &config, RuleForm::BeforeQuicPort);
            assert_eq!(verify(&current, mode, &config), Ok(7));
            assert_eq!(
                verify_form(
                    &earlier,
                    mode,
                    &config,
                    CommentSource::Json,
                    RuleForm::BeforeQuicPort
                ),
                Ok(7)
            );
            // Each table is its own form and not the other.
            assert_eq!(
                verify(&earlier, mode, &config),
                Err("nft rule content mismatch")
            );
            assert!(
                verify_form(
                    &current,
                    mode,
                    &config,
                    CommentSource::Json,
                    RuleForm::BeforeQuicPort
                )
                .is_err()
            );
            // What differs is the port, from the local ranges, over UDP.
            let rules = |document: &str| {
                serde_json::from_str::<Value>(document).unwrap()["nftables"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|entry| entry.get("rule").is_some())
                    .count()
            };
            assert_eq!(rules(&current), rules(&earlier) + 2);
        }
        // The other modes have one form.
        for mode in [
            FirewallMode::Baseline,
            FirewallMode::Moderate,
            FirewallMode::Paranoid,
        ] {
            let config = ports(None);
            assert_eq!(
                observed_document(mode, &config, RuleForm::BeforeQuicPort),
                observed(mode, &config)
            );
        }
    }

    #[test]
    fn webrtc_media_is_verified_exactly_in_restrictive_modes_only() {
        let media = ports(Some((20000, 20099)));
        for mode in [FirewallMode::Restricted, FirewallMode::Paranoid] {
            let document = observed(mode, &media);
            assert!(document.contains("\"range\":[20000,20099]"));
            assert_eq!(verify(&document, mode, &media), Ok(7));
            // The same ruleset without the allowance, or with another range,
            // does not match the configured policy.
            assert!(verify(&document, mode, &ports(None)).is_err());
            assert!(verify(&document, mode, &ports(Some((20000, 20100)))).is_err());
            assert!(verify(&observed(mode, &ports(None)), mode, &media).is_err());
        }
        for mode in [FirewallMode::Baseline, FirewallMode::Moderate] {
            let document = observed(mode, &media);
            assert!(!document.contains("range"));
            assert_eq!(verify(&document, mode, &media), Ok(7));
        }
    }
}
