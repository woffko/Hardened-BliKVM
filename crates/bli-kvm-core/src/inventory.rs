use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const RUNTIME_INVENTORY_SCHEMA_VERSION: u16 = 1;

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
}
