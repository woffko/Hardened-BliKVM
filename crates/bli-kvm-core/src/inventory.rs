use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const RUNTIME_INVENTORY_SCHEMA_VERSION: u16 = 3;

const MIN_RTC_UNIX_SECONDS: i64 = 946_684_800;
const MAX_RTC_UNIX_SECONDS: i64 = 4_102_444_799;
const MAX_RTC_OFFSET_SECONDS: i64 = 315_576_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryBackend {
    KvmdTransition,
    Native,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComponentObservation {
    pub enabled: Option<bool>,
    pub online: Option<bool>,
    pub busy: Option<bool>,
    pub connected: Option<bool>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeVersions {
    pub kvmd: Option<String>,
    pub streamer: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThermalObservation {
    pub temperature_millidegrees_celsius: Option<i32>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RtcObservation {
    pub unix_seconds: Option<i64>,
    /// External RTC Unix seconds minus system Unix seconds.
    pub offset_from_system_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeInventory {
    pub schema_version: u16,
    pub platform_id: String,
    pub backend: InventoryBackend,
    pub versions: RuntimeVersions,
    pub video: ComponentObservation,
    pub hid: ComponentObservation,
    pub power: ComponentObservation,
    pub virtual_media: ComponentObservation,
    pub thermal: ThermalObservation,
    pub rtc: RtcObservation,
    pub power_led: Option<bool>,
    pub janus_enabled: Option<bool>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum InventoryError {
    #[error("unsupported runtime inventory schema version {0}")]
    UnsupportedSchemaVersion(u16),
    #[error("runtime inventory platform ID is empty")]
    EmptyPlatformId,
    #[error("runtime inventory version field is invalid")]
    InvalidVersion,
    #[error("runtime inventory temperature is outside the accepted range")]
    InvalidTemperature,
    #[error("runtime inventory RTC observation is invalid")]
    InvalidRtc,
}

impl RuntimeInventory {
    pub fn validate(&self) -> Result<(), InventoryError> {
        if self.schema_version != RUNTIME_INVENTORY_SCHEMA_VERSION {
            return Err(InventoryError::UnsupportedSchemaVersion(
                self.schema_version,
            ));
        }
        if self.platform_id.trim().is_empty() {
            return Err(InventoryError::EmptyPlatformId);
        }
        for version in [
            self.versions.kvmd.as_deref(),
            self.versions.streamer.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if version.is_empty()
                || version.len() > 64
                || version.chars().any(|character| character.is_control())
            {
                return Err(InventoryError::InvalidVersion);
            }
        }
        if self
            .thermal
            .temperature_millidegrees_celsius
            .is_some_and(|value| !(-40_000..=150_000).contains(&value))
        {
            return Err(InventoryError::InvalidTemperature);
        }
        match (self.rtc.unix_seconds, self.rtc.offset_from_system_seconds) {
            (None, None) => {}
            (Some(unix_seconds), Some(offset))
                if (MIN_RTC_UNIX_SECONDS..=MAX_RTC_UNIX_SECONDS).contains(&unix_seconds)
                    && offset.unsigned_abs() <= MAX_RTC_OFFSET_SECONDS as u64 => {}
            _ => return Err(InventoryError::InvalidRtc),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inventory() -> RuntimeInventory {
        RuntimeInventory {
            schema_version: RUNTIME_INVENTORY_SCHEMA_VERSION,
            platform_id: "bli-h616-pikvm".into(),
            backend: InventoryBackend::KvmdTransition,
            versions: RuntimeVersions {
                kvmd: Some("3.291".into()),
                streamer: Some("6.11".into()),
            },
            video: ComponentObservation {
                online: Some(true),
                ..ComponentObservation::default()
            },
            hid: ComponentObservation::default(),
            power: ComponentObservation::default(),
            virtual_media: ComponentObservation::default(),
            thermal: ThermalObservation::default(),
            rtc: RtcObservation::default(),
            power_led: None,
            janus_enabled: Some(false),
        }
    }

    #[test]
    fn sanitized_inventory_is_valid_and_has_no_identifier_fields() {
        let inventory = inventory();
        assert_eq!(inventory.validate(), Ok(()));
        let json = serde_json::to_string(&inventory).unwrap();
        for forbidden in ["password", "credential", "serial", "username", "10.0."] {
            assert!(!json.to_ascii_lowercase().contains(forbidden));
        }
    }

    #[test]
    fn rejects_control_characters_in_versions() {
        let mut inventory = inventory();
        inventory.versions.kvmd = Some("3.291\nleak".into());
        assert_eq!(inventory.validate(), Err(InventoryError::InvalidVersion));
    }

    #[test]
    fn thermal_observation_is_bounded() {
        let mut inventory = inventory();
        inventory.thermal.temperature_millidegrees_celsius = Some(42_125);
        assert_eq!(inventory.validate(), Ok(()));
        inventory.thermal.temperature_millidegrees_celsius = Some(150_001);
        assert_eq!(
            inventory.validate(),
            Err(InventoryError::InvalidTemperature)
        );
    }

    #[test]
    fn rtc_observation_is_paired_and_bounded() {
        let mut inventory = inventory();
        inventory.rtc.unix_seconds = Some(1_759_489_393);
        inventory.rtc.offset_from_system_seconds = Some(-28_647_995);
        assert_eq!(inventory.validate(), Ok(()));

        inventory.rtc.offset_from_system_seconds = None;
        assert_eq!(inventory.validate(), Err(InventoryError::InvalidRtc));
        inventory.rtc.offset_from_system_seconds = Some(-28_647_995);
        inventory.rtc.unix_seconds = Some(MAX_RTC_UNIX_SECONDS + 1);
        assert_eq!(inventory.validate(), Err(InventoryError::InvalidRtc));
    }
}
