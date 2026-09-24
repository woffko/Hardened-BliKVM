//! Verify the complete owned-table policy against observed nft JSON (schema 1).
use crate::firewall_rules::{FirewallMode, FirewallPorts, render};
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

fn expected_rules(mode: FirewallMode, ports: &FirewallPorts<'_>) -> Vec<Value> {
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
        let v4 = json!({"set":[{"prefix":{"addr":"10.0.0.0","len":8}}, {"prefix":{"addr":"172.16.0.0","len":12}}, {"prefix":{"addr":"192.168.0.0","len":16}}, {"prefix":{"addr":"169.254.0.0","len":16}}]});
        let v6 = json!({"set":[{"prefix":{"addr":"fc00::","len":7}}, {"prefix":{"addr":"fe80::","len":10}}]});
        for (protocol, addresses) in [("ip", v4), ("ip6", v6)] {
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

pub fn verify(
    document: &str,
    mode: FirewallMode,
    ports: &FirewallPorts<'_>,
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
                    if handle.is_some()
                        || Value::Object(body)
                            != json!({"family":"inet","name":"hardened-blikvm","comment":format!("hardened-blikvm-owner-v2:{}",mode.as_str())})
                    {
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
    let mut expected = Value::Array(expected_rules(mode, ports));
    normalize(&mut actual);
    normalize(&mut expected);
    if actual != expected {
        return Err("nft rule content mismatch");
    }
    handle.ok_or("missing owned table")
}
