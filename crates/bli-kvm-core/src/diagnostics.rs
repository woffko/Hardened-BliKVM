use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ComponentObservation, RuntimeInventory};

pub const DIAGNOSTICS_VIEW_SCHEMA_VERSION: u16 = 1;

const MIN_TEMPERATURE_MILLIDEGREES_CELSIUS: i32 = -40_000;
const MAX_TEMPERATURE_MILLIDEGREES_CELSIUS: i32 = 150_000;
const MIN_RTC_UNIX_SECONDS: i64 = 946_684_800;
const MAX_RTC_UNIX_SECONDS: i64 = 4_102_444_799;
const MAX_RTC_OFFSET_SECONDS: u64 = 315_576_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsAvailability {
    CurrentOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticMetricState {
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticBooleanState {
    Active,
    Inactive,
    Unavailable,
}

impl From<Option<bool>> for DiagnosticBooleanState {
    fn from(value: Option<bool>) -> Self {
        match value {
            Some(true) => Self::Active,
            Some(false) => Self::Inactive,
            None => Self::Unavailable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticTemperature {
    pub state: DiagnosticMetricState,
    pub millidegrees_celsius: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticRtc {
    pub state: DiagnosticMetricState,
    pub unix_seconds: Option<i64>,
    pub offset_from_system_seconds: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticComponentHealth {
    pub enabled: DiagnosticBooleanState,
    pub online: DiagnosticBooleanState,
    pub busy: DiagnosticBooleanState,
    pub connected: DiagnosticBooleanState,
}

impl From<&ComponentObservation> for DiagnosticComponentHealth {
    fn from(observation: &ComponentObservation) -> Self {
        Self {
            enabled: observation.enabled.into(),
            online: observation.online.into(),
            busy: observation.busy.into(),
            connected: observation.connected.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticComponents {
    pub video: DiagnosticComponentHealth,
    pub hid: DiagnosticComponentHealth,
    pub power: DiagnosticComponentHealth,
    pub virtual_media: DiagnosticComponentHealth,
    pub power_led: DiagnosticBooleanState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticsCurrent {
    pub temperature: DiagnosticTemperature,
    pub rtc: DiagnosticRtc,
    pub components: DiagnosticComponents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticHistoryState {
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticHistoryReason {
    PersistenceNotProvisioned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticHistory {
    pub state: DiagnosticHistoryState,
    pub reason: DiagnosticHistoryReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticsView {
    pub schema_version: u16,
    pub availability: DiagnosticsAvailability,
    pub current: DiagnosticsCurrent,
    pub history: DiagnosticHistory,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticsViewError {
    #[error("runtime inventory is invalid for diagnostics")]
    InvalidInventory,
    #[error("unsupported diagnostics view schema version")]
    UnsupportedSchemaVersion,
    #[error("diagnostics view metric state is invalid")]
    InvalidMetricState,
}

impl DiagnosticsView {
    pub fn from_inventory(inventory: &RuntimeInventory) -> Result<Self, DiagnosticsViewError> {
        inventory
            .validate()
            .map_err(|_| DiagnosticsViewError::InvalidInventory)?;
        let temperature = match inventory.thermal.temperature_millidegrees_celsius {
            Some(value) => DiagnosticTemperature {
                state: DiagnosticMetricState::Available,
                millidegrees_celsius: Some(value),
            },
            None => DiagnosticTemperature {
                state: DiagnosticMetricState::Unavailable,
                millidegrees_celsius: None,
            },
        };
        let rtc = match (
            inventory.rtc.unix_seconds,
            inventory.rtc.offset_from_system_seconds,
        ) {
            (Some(unix_seconds), Some(offset)) => DiagnosticRtc {
                state: DiagnosticMetricState::Available,
                unix_seconds: Some(unix_seconds),
                offset_from_system_seconds: Some(offset),
            },
            (None, None) => DiagnosticRtc {
                state: DiagnosticMetricState::Unavailable,
                unix_seconds: None,
                offset_from_system_seconds: None,
            },
            _ => return Err(DiagnosticsViewError::InvalidInventory),
        };
        let view = Self {
            schema_version: DIAGNOSTICS_VIEW_SCHEMA_VERSION,
            availability: DiagnosticsAvailability::CurrentOnly,
            current: DiagnosticsCurrent {
                temperature,
                rtc,
                components: DiagnosticComponents {
                    video: (&inventory.video).into(),
                    hid: (&inventory.hid).into(),
                    power: (&inventory.power).into(),
                    virtual_media: (&inventory.virtual_media).into(),
                    power_led: inventory.power_led.into(),
                },
            },
            history: DiagnosticHistory {
                state: DiagnosticHistoryState::Unavailable,
                reason: DiagnosticHistoryReason::PersistenceNotProvisioned,
            },
        };
        view.validate()?;
        Ok(view)
    }

    pub fn validate(&self) -> Result<(), DiagnosticsViewError> {
        if self.schema_version != DIAGNOSTICS_VIEW_SCHEMA_VERSION {
            return Err(DiagnosticsViewError::UnsupportedSchemaVersion);
        }
        match (
            self.current.temperature.state,
            self.current.temperature.millidegrees_celsius,
        ) {
            (DiagnosticMetricState::Unavailable, None) => {}
            (DiagnosticMetricState::Available, Some(value))
                if (MIN_TEMPERATURE_MILLIDEGREES_CELSIUS
                    ..=MAX_TEMPERATURE_MILLIDEGREES_CELSIUS)
                    .contains(&value) => {}
            _ => return Err(DiagnosticsViewError::InvalidMetricState),
        }
        match (
            self.current.rtc.state,
            self.current.rtc.unix_seconds,
            self.current.rtc.offset_from_system_seconds,
        ) {
            (DiagnosticMetricState::Unavailable, None, None) => {}
            (DiagnosticMetricState::Available, Some(unix_seconds), Some(offset))
                if (MIN_RTC_UNIX_SECONDS..=MAX_RTC_UNIX_SECONDS).contains(&unix_seconds)
                    && offset.unsigned_abs() <= MAX_RTC_OFFSET_SECONDS => {}
            _ => return Err(DiagnosticsViewError::InvalidMetricState),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ComponentObservation, InventoryBackend, RUNTIME_INVENTORY_SCHEMA_VERSION, RtcObservation,
        RuntimeVersions, ThermalObservation,
    };

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
                enabled: Some(true),
                online: Some(true),
                busy: Some(false),
                connected: None,
            },
            hid: ComponentObservation {
                online: Some(true),
                ..ComponentObservation::default()
            },
            power: ComponentObservation::default(),
            virtual_media: ComponentObservation {
                enabled: Some(true),
                online: Some(true),
                busy: Some(false),
                connected: Some(false),
            },
            thermal: ThermalObservation {
                temperature_millidegrees_celsius: Some(42_125),
            },
            rtc: RtcObservation {
                unix_seconds: Some(1_759_489_393),
                offset_from_system_seconds: Some(-28_647_995),
            },
            power_led: Some(true),
            janus_enabled: Some(false),
        }
    }

    #[test]
    fn sanitized_inventory_maps_to_current_only_view() {
        let view = DiagnosticsView::from_inventory(&inventory()).unwrap();
        assert_eq!(view.validate(), Ok(()));
        assert_eq!(view.availability, DiagnosticsAvailability::CurrentOnly);
        assert_eq!(
            view.current.temperature,
            DiagnosticTemperature {
                state: DiagnosticMetricState::Available,
                millidegrees_celsius: Some(42_125),
            }
        );
        assert_eq!(
            view.current.components.video.online,
            DiagnosticBooleanState::Active
        );
        assert_eq!(
            view.current.components.video.connected,
            DiagnosticBooleanState::Unavailable
        );
        assert_eq!(
            view.history.reason,
            DiagnosticHistoryReason::PersistenceNotProvisioned
        );
    }

    #[test]
    fn absent_temperature_and_rtc_are_explicitly_unavailable() {
        let mut inventory = inventory();
        inventory.thermal = ThermalObservation::default();
        inventory.rtc = RtcObservation::default();
        let view = DiagnosticsView::from_inventory(&inventory).unwrap();
        assert_eq!(
            view.current.temperature.state,
            DiagnosticMetricState::Unavailable
        );
        assert_eq!(view.current.temperature.millidegrees_celsius, None);
        assert_eq!(view.current.rtc.state, DiagnosticMetricState::Unavailable);
        assert_eq!(view.current.rtc.unix_seconds, None);
    }

    #[test]
    fn invalid_inventory_and_view_state_fail_closed() {
        let mut invalid_inventory = inventory();
        invalid_inventory.thermal.temperature_millidegrees_celsius = Some(150_001);
        assert_eq!(
            DiagnosticsView::from_inventory(&invalid_inventory),
            Err(DiagnosticsViewError::InvalidInventory)
        );

        let mut view = DiagnosticsView::from_inventory(&inventory()).unwrap();
        view.current.temperature.state = DiagnosticMetricState::Unavailable;
        assert_eq!(
            view.validate(),
            Err(DiagnosticsViewError::InvalidMetricState)
        );
    }

    #[test]
    fn serialized_view_excludes_identity_version_log_and_persistence_payloads() {
        let view = DiagnosticsView::from_inventory(&inventory()).unwrap();
        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains("\"availability\":\"current_only\""));
        assert!(json.contains("\"reason\":\"persistence_not_provisioned\""));
        for forbidden in [
            "bli-h616-pikvm",
            "3.291",
            "6.11",
            "bundle",
            "binding",
            "instance",
            "window",
            "password",
            "credential",
            "path",
            "serial",
            "address",
            "raw",
            "log",
        ] {
            assert!(!json.to_ascii_lowercase().contains(forbidden));
        }
    }

    #[test]
    fn decoding_rejects_unknown_fields_and_invalid_metric_coherence() {
        let view = DiagnosticsView::from_inventory(&inventory()).unwrap();
        let mut value = serde_json::to_value(view).unwrap();
        value["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<DiagnosticsView>(value).is_err());

        let mut value =
            serde_json::to_value(DiagnosticsView::from_inventory(&inventory()).unwrap()).unwrap();
        value["current"]["temperature"]["state"] = serde_json::json!("unavailable");
        let decoded: DiagnosticsView = serde_json::from_value(value).unwrap();
        assert_eq!(
            decoded.validate(),
            Err(DiagnosticsViewError::InvalidMetricState)
        );
    }
}
