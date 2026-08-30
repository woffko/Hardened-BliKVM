use bli_kvm_core::{
    ComponentObservation, InventoryBackend, RUNTIME_INVENTORY_SCHEMA_VERSION, RuntimeInventory,
    RuntimeVersions,
};
use bli_kvm_kvmd_client::KvmdInventory;

const PLATFORM_ID: &str = "bli-h616-pikvm";

/// Remove KVMD-specific structure and retain only the public runtime facts.
pub fn sanitize_kvmd_inventory(source: KvmdInventory) -> RuntimeInventory {
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
        let inventory = sanitize_kvmd_inventory(source);
        assert_eq!(inventory.validate(), Ok(()));
        assert_eq!(inventory.video.online, Some(true));
        assert_eq!(inventory.virtual_media.connected, Some(false));
        assert_eq!(inventory.power_led, Some(true));
        assert_eq!(inventory.janus_enabled, Some(false));
    }
}
