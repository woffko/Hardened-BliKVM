use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KvmdVersionInfo {
    pub version: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KvmdSystemInfo {
    pub kvmd: KvmdVersionInfo,
    pub streamer: KvmdVersionInfo,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KvmdEndpointState {
    pub enabled: Option<bool>,
    pub online: Option<bool>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KvmdExtrasInfo {
    pub janus: KvmdEndpointState,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KvmdInfoState {
    pub system: KvmdSystemInfo,
    pub extras: KvmdExtrasInfo,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KvmdAtxLeds {
    pub power: Option<bool>,
    pub hdd: Option<bool>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KvmdAtxState {
    pub enabled: Option<bool>,
    pub busy: Option<bool>,
    pub leds: KvmdAtxLeds,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KvmdHidState {
    pub enabled: Option<bool>,
    pub online: Option<bool>,
    pub busy: Option<bool>,
    pub connected: Option<bool>,
    pub keyboard: KvmdEndpointState,
    pub mouse: KvmdEndpointState,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KvmdMsdDrive {
    pub connected: Option<bool>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KvmdMsdState {
    pub enabled: Option<bool>,
    pub online: Option<bool>,
    pub busy: Option<bool>,
    pub drive: KvmdMsdDrive,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KvmdStreamerSource {
    pub online: Option<bool>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KvmdStreamerInfo {
    pub source: KvmdStreamerSource,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KvmdStreamerState {
    pub streamer: KvmdStreamerInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvmdInventory {
    pub info: KvmdInfoState,
    pub atx: KvmdAtxState,
    pub hid: KvmdHidState,
    pub msd: KvmdMsdState,
    pub streamer: KvmdStreamerState,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_shapes_are_typed_without_identifiers() {
        let atx: KvmdAtxState = serde_json::from_value(serde_json::json!({
            "enabled": true,
            "busy": false,
            "leds": {"power": true}
        }))
        .unwrap();
        assert_eq!(atx.enabled, Some(true));
        assert_eq!(atx.busy, Some(false));
        assert_eq!(atx.leds.power, Some(true));

        let info: KvmdInfoState = serde_json::from_value(serde_json::json!({
            "system": {
                "kvmd": {"version": "3.291"},
                "streamer": {"version": "6.11"}
            },
            "extras": {"janus": {"enabled": false}}
        }))
        .unwrap();
        assert_eq!(info.system.kvmd.version.as_deref(), Some("3.291"));
        assert_eq!(info.extras.janus.enabled, Some(false));
    }
}
