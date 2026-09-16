//! BliKVM v4 H616 platform description.
//!
//! The current manifest is intentionally evidence-aware. Reference-stack
//! runtime evidence is retained without claiming that this Rust backend
//! exposes an adapter. User reports are never promoted to runtime or physical
//! verification merely because an endpoint is reachable.

mod gpio;
mod gpio_driver;
mod hardware;
mod inventory;
mod rtc;
mod sysfs;
mod thermal;

pub use gpio::{
    BOARD_CONSUMER, LineDirection, LineOwner, LinePlanningError, LineRequest, MAX_PLANNED_LINES,
    current_owner, hardened_board_request, plan_requests, role_direction,
};
pub use gpio_driver::{ApplyError, GpioDriver, apply_plan, release_plan};
pub use hardware::{
    GpioLineEvidence, GpioRole, H616_ATX_POWER_LONG_MS, H616_ATX_POWER_SHORT_MS, H616_ATX_RESET_MS,
    H616_VENDOR_PROFILE, H616VendorProfile, RtcEvidence, VENDOR_SOURCE_REVISION,
};
pub use inventory::BliPlatformObservations;
#[cfg(feature = "kvmd-inventory")]
pub use inventory::{sanitize_kvmd_inventory, sanitize_kvmd_inventory_with_observations};
pub use rtc::{RtcReadError, RtcReadout, read_h616_external_rtc};
pub use thermal::{ThermalReadError, read_h616_temperature_millidegrees};

use std::collections::BTreeMap;

use bli_kvm_core::{
    BoardCapabilities, CAPABILITY_SCHEMA_VERSION, CapabilityManifest, CapabilityProvider,
    EvidenceLevel, FeatureStatus, HidCapabilities, HidMode, PlatformIdentity, PowerAction,
    PowerCapabilities, ProductIdentity, SystemCapabilities, VideoCapabilities, VideoTransport,
    VirtualMediaCapabilities, VirtualMediaMode,
};

#[derive(Debug, Default, Clone, Copy)]
pub struct BliH616PikvmPlatform;

impl CapabilityProvider for BliH616PikvmPlatform {
    fn capabilities(&self) -> CapabilityManifest {
        let user_reported = EvidenceLevel::UserReported;
        let runtime_verified = EvidenceLevel::RuntimeVerified;
        let source_verified = EvidenceLevel::SourceVerified;
        let mut board_features = BTreeMap::new();
        for name in ["fan", "oled", "physical_buttons", "serial_switch"] {
            board_features.insert(
                name.to_string(),
                FeatureStatus::not_exposed(source_verified),
            );
        }
        board_features.insert("thermal".into(), FeatureStatus::read_only(runtime_verified));
        board_features.insert("rtc".into(), FeatureStatus::read_only(runtime_verified));
        board_features.insert(
            "system_settings".into(),
            FeatureStatus::read_only(runtime_verified),
        );
        board_features.insert(
            "system_time".into(),
            FeatureStatus::read_only(runtime_verified),
        );
        board_features.insert(
            "system_log".into(),
            FeatureStatus::read_only(runtime_verified),
        );
        board_features.insert(
            "system_network".into(),
            FeatureStatus::not_exposed(source_verified),
        );
        board_features.insert(
            "system_firewall".into(),
            FeatureStatus::not_exposed(source_verified),
        );

        CapabilityManifest {
            schema_version: CAPABILITY_SCHEMA_VERSION,
            product: ProductIdentity {
                name: "BLi-KVM".into(),
                short_name: "BLi-KVM".into(),
                tagline: "Hardened with Rust".into(),
            },
            platform: PlatformIdentity {
                id: "bli-h616-pikvm".into(),
                board: "BliKVM v4 H616".into(),
                architecture: "aarch64".into(),
                operating_system: "PiKVM/KVMD on Armbian".into(),
            },
            video: VideoCapabilities {
                status: FeatureStatus::read_only(runtime_verified),
                transports: vec![VideoTransport::Mjpeg],
                preferred_transport: Some(VideoTransport::Mjpeg),
                max_width: None,
                max_height: None,
                audio: FeatureStatus::unavailable(),
            },
            hid: HidCapabilities {
                status: FeatureStatus::not_exposed(runtime_verified),
                modes: vec![
                    HidMode::Keyboard,
                    HidMode::AbsoluteMouse,
                    HidMode::RelativeMouse,
                ],
            },
            power: PowerCapabilities {
                status: FeatureStatus::not_exposed(runtime_verified),
                actions: vec![
                    PowerAction::PowerShort,
                    PowerAction::PowerLong,
                    PowerAction::Reset,
                ],
                led_state: FeatureStatus::read_only(runtime_verified),
            },
            virtual_media: VirtualMediaCapabilities {
                status: FeatureStatus::read_only(runtime_verified),
                modes: vec![VirtualMediaMode::CdRom, VirtualMediaMode::RemovableDisk],
                resumable_upload: FeatureStatus::unavailable(),
                remote_download: FeatureStatus::unavailable(),
            },
            board: BoardCapabilities {
                features: board_features,
            },
            system: SystemCapabilities {
                status: FeatureStatus::read_only(user_reported),
                read_only_rootfs: true,
                maintenance_write_mode: true,
                actions: FeatureStatus::unavailable(),
                signed_application_updates: FeatureStatus::unavailable(),
                signed_system_updates: FeatureStatus::unavailable(),
                automatic_rollback: FeatureStatus::unavailable(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticated_baseline_is_valid_but_not_overstated() {
        let manifest = BliH616PikvmPlatform.capabilities();
        assert_eq!(manifest.validate(), Ok(()));
        assert_eq!(
            manifest.video.status.evidence,
            EvidenceLevel::RuntimeVerified
        );
        assert_eq!(
            manifest.video.status.availability,
            bli_kvm_core::Availability::ReadOnly
        );
        assert!(manifest.video.status.is_available());
        assert_eq!(manifest.video.transports, vec![VideoTransport::Mjpeg]);
        assert!(
            !manifest
                .video
                .transports
                .contains(&VideoTransport::WebRtcJanus)
        );
        assert_eq!(
            manifest.system.signed_application_updates,
            FeatureStatus::unavailable()
        );
        assert!(!manifest.hid.status.is_available());
        assert!(!manifest.power.status.is_available());
        assert_eq!(
            manifest.power.led_state.availability,
            bli_kvm_core::Availability::ReadOnly
        );
        assert_eq!(
            manifest.power.led_state.evidence,
            EvidenceLevel::RuntimeVerified
        );
        assert_eq!(
            manifest.virtual_media.status.availability,
            bli_kvm_core::Availability::ReadOnly
        );
        assert_eq!(
            manifest.board.features["thermal"],
            FeatureStatus::read_only(EvidenceLevel::RuntimeVerified)
        );
        assert_eq!(
            manifest.board.features["fan"],
            FeatureStatus::not_exposed(EvidenceLevel::SourceVerified)
        );
        assert_eq!(
            manifest.board.features["rtc"],
            FeatureStatus::read_only(EvidenceLevel::RuntimeVerified)
        );
        assert_eq!(
            manifest.board.features["system_settings"],
            FeatureStatus::read_only(EvidenceLevel::RuntimeVerified)
        );
        assert_eq!(
            manifest.board.features["system_time"],
            FeatureStatus::read_only(EvidenceLevel::RuntimeVerified)
        );
        assert_eq!(
            manifest.board.features["system_log"],
            FeatureStatus::read_only(EvidenceLevel::RuntimeVerified)
        );
        assert_eq!(
            manifest.board.features["system_network"],
            FeatureStatus::not_exposed(EvidenceLevel::SourceVerified)
        );
        assert_eq!(
            manifest.board.features["system_firewall"],
            FeatureStatus::not_exposed(EvidenceLevel::SourceVerified)
        );
    }

    #[test]
    fn public_manifest_contains_no_endpoint_or_credential_fields() {
        let json = serde_json::to_string(&BliH616PikvmPlatform.capabilities()).unwrap();
        for forbidden in ["password", "credential", "10.0.", "ssh://", "https://"] {
            assert!(!json.to_ascii_lowercase().contains(forbidden));
        }
    }
}
