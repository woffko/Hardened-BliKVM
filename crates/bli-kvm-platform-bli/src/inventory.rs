use bli_kvm_core::{
    ComponentObservation, InventoryBackend, RUNTIME_INVENTORY_SCHEMA_VERSION, RtcObservation,
    RuntimeInventory, RuntimeVersions, ThermalObservation,
};
use bli_kvm_kvmd_client::KvmdInventory;

use crate::RtcReadout;

const PLATFORM_ID: &str = "bli-h616-pikvm";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BliPlatformObservations {
    pub temperature_millidegrees_celsius: Option<i32>,
    pub external_rtc: Option<RtcReadout>,
}

/// Remove KVMD-specific structure and retain only the public runtime facts.
pub fn sanitize_kvmd_inventory(source: KvmdInventory) -> RuntimeInventory {
    sanitize_kvmd_inventory_with_observations(source, BliPlatformObservations::default())
}

pub fn sanitize_kvmd_inventory_with_observations(
    source: KvmdInventory,
    observations: BliPlatformObservations,
) -> RuntimeInventory {
    let inventory = RuntimeInventory {
        schema_version: RUNTIME_INVENTORY_SCHEMA_VERSION,
        platform_id: PLATFORM_ID.into(),
        backend: InventoryBackend::KvmdTransition,
        versions: RuntimeVersions {
            kvmd: source.info.system.kvmd.version,
            streamer: source.info.system.streamer.version,
        },
        video: ComponentObservation {
            online: source.streamer.streamer.source.online,
            ..ComponentObservation::default()
        },
        hid: ComponentObservation {
            enabled: source.hid.enabled,
            online: source.hid.online,
            busy: source.hid.busy,
            connected: source.hid.connected,
        },
        power: ComponentObservation {
            enabled: source.atx.enabled,
            busy: source.atx.busy,
            ..ComponentObservation::default()
        },
        virtual_media: ComponentObservation {
            enabled: source.msd.enabled,
            online: source.msd.online,
            busy: source.msd.busy,
            connected: source.msd.drive.connected,
        },
        thermal: ThermalObservation {
            temperature_millidegrees_celsius: observations.temperature_millidegrees_celsius,
        },
        rtc: RtcObservation {
            unix_seconds: observations.external_rtc.map(|rtc| rtc.unix_seconds),
            offset_from_system_seconds: observations
                .external_rtc
                .map(|rtc| rtc.offset_from_system_seconds),
        },
        power_led: source.atx.leds.power,
        janus_enabled: source.info.extras.janus.enabled,
    };
    debug_assert_eq!(inventory.validate(), Ok(()));
    inventory
}

#[cfg(test)]
mod tests {
    use bli_kvm_kvmd_client::{
        KvmdAtxLeds, KvmdAtxState, KvmdEndpointState, KvmdExtrasInfo, KvmdHidState, KvmdInfoState,
        KvmdMsdDrive, KvmdMsdState, KvmdStreamerInfo, KvmdStreamerSource, KvmdStreamerState,
        KvmdSystemInfo, KvmdVersionInfo,
    };

    use super::*;

    #[test]
    fn maps_only_sanitized_runtime_facts() {
        let source = KvmdInventory {
            info: KvmdInfoState {
                system: KvmdSystemInfo {
                    kvmd: KvmdVersionInfo {
                        version: Some("3.291".into()),
                    },
                    streamer: KvmdVersionInfo {
                        version: Some("6.11".into()),
                    },
                },
                extras: KvmdExtrasInfo {
                    janus: KvmdEndpointState {
                        enabled: Some(false),
                        ..KvmdEndpointState::default()
                    },
                },
            },
            atx: KvmdAtxState {
                enabled: Some(true),
                busy: Some(false),
                leds: KvmdAtxLeds {
                    power: Some(true),
                    hdd: None,
                },
            },
            hid: KvmdHidState {
                online: Some(true),
                ..KvmdHidState::default()
            },
            msd: KvmdMsdState {
                enabled: Some(true),
                online: Some(true),
                busy: Some(false),
                drive: KvmdMsdDrive {
                    connected: Some(false),
                },
            },
            streamer: KvmdStreamerState {
                streamer: KvmdStreamerInfo {
                    source: KvmdStreamerSource { online: Some(true) },
                },
            },
        };
        let inventory = sanitize_kvmd_inventory(source.clone());
        assert_eq!(inventory.validate(), Ok(()));
        assert_eq!(inventory.video.online, Some(true));
        assert_eq!(inventory.virtual_media.connected, Some(false));
        assert_eq!(inventory.power_led, Some(true));
        assert_eq!(inventory.janus_enabled, Some(false));
        assert_eq!(inventory.thermal.temperature_millidegrees_celsius, None);
        assert_eq!(inventory.rtc, RtcObservation::default());

        let inventory = sanitize_kvmd_inventory_with_observations(
            source,
            BliPlatformObservations {
                temperature_millidegrees_celsius: Some(42_125),
                external_rtc: Some(RtcReadout {
                    unix_seconds: 1_759_489_393,
                    offset_from_system_seconds: -28_647_995,
                }),
            },
        );
        assert_eq!(
            inventory.thermal.temperature_millidegrees_celsius,
            Some(42_125)
        );
        assert_eq!(inventory.rtc.unix_seconds, Some(1_759_489_393));
        assert_eq!(inventory.rtc.offset_from_system_seconds, Some(-28_647_995));
    }
}
