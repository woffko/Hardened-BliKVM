use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::CAPABILITY_SCHEMA_VERSION;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Unavailable,
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceLevel {
    None,
    UserReported,
    SourceVerified,
    RuntimeVerified,
    PhysicalVerified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureStatus {
    pub availability: Availability,
    pub evidence: EvidenceLevel,
}

impl FeatureStatus {
    pub const fn unavailable() -> Self {
        Self {
            availability: Availability::Unavailable,
            evidence: EvidenceLevel::None,
        }
    }

    /// The underlying feature was observed, but this backend does not expose it.
    ///
    /// This is deliberately different from `unavailable()`: evidence about a
    /// reference stack or the hardware must not be promoted to API support.
    pub const fn not_exposed(evidence: EvidenceLevel) -> Self {
        Self {
            availability: Availability::Unavailable,
            evidence,
        }
    }

    pub const fn read_only(evidence: EvidenceLevel) -> Self {
        Self {
            availability: Availability::ReadOnly,
            evidence,
        }
    }

    pub const fn read_write(evidence: EvidenceLevel) -> Self {
        Self {
            availability: Availability::ReadWrite,
            evidence,
        }
    }

    pub const fn is_available(self) -> bool {
        !matches!(self.availability, Availability::Unavailable)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductIdentity {
    pub name: String,
    pub short_name: String,
    pub tagline: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformIdentity {
    pub id: String,
    pub board: String,
    pub architecture: String,
    pub operating_system: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoTransport {
    Mjpeg,
    H264Direct,
    WebRtcJanus,
    WebRtcNative,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoCapabilities {
    pub status: FeatureStatus,
    pub transports: Vec<VideoTransport>,
    pub preferred_transport: Option<VideoTransport>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub audio: FeatureStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HidMode {
    Keyboard,
    AbsoluteMouse,
    RelativeMouse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HidCapabilities {
    pub status: FeatureStatus,
    pub modes: Vec<HidMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerAction {
    PowerShort,
    PowerLong,
    Reset,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerCapabilities {
    pub status: FeatureStatus,
    pub actions: Vec<PowerAction>,
    pub led_state: FeatureStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VirtualMediaMode {
    CdRom,
    RemovableDisk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualMediaCapabilities {
    pub status: FeatureStatus,
    pub modes: Vec<VirtualMediaMode>,
    pub resumable_upload: FeatureStatus,
    pub remote_download: FeatureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardCapabilities {
    pub features: BTreeMap<String, FeatureStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemCapabilities {
    pub status: FeatureStatus,
    pub read_only_rootfs: bool,
    pub maintenance_write_mode: bool,
    pub signed_application_updates: FeatureStatus,
    pub signed_system_updates: FeatureStatus,
    pub automatic_rollback: FeatureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityManifest {
    pub schema_version: u16,
    pub product: ProductIdentity,
    pub platform: PlatformIdentity,
    pub video: VideoCapabilities,
    pub hid: HidCapabilities,
    pub power: PowerCapabilities,
    pub virtual_media: VirtualMediaCapabilities,
    pub board: BoardCapabilities,
    pub system: SystemCapabilities,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CapabilityError {
    #[error("unsupported capability schema version {0}")]
    UnsupportedSchemaVersion(u16),
    #[error("required capability field {0} is empty")]
    EmptyField(&'static str),
    #[error("available video capability has no transport")]
    MissingVideoTransport,
    #[error("preferred video transport is not in the advertised transport list")]
    InvalidPreferredVideoTransport,
    #[error("available HID capability has no mode")]
    MissingHidMode,
    #[error("available power capability has no action")]
    MissingPowerAction,
    #[error("available virtual-media capability has no mode")]
    MissingVirtualMediaMode,
    #[error("maintenance write mode requires a read-only root filesystem")]
    InvalidMaintenanceWriteMode,
}

impl CapabilityManifest {
    pub fn validate(&self) -> Result<(), CapabilityError> {
        if self.schema_version != CAPABILITY_SCHEMA_VERSION {
            return Err(CapabilityError::UnsupportedSchemaVersion(
                self.schema_version,
            ));
        }
        for (name, value) in [
            ("product.name", self.product.name.as_str()),
            ("product.shortName", self.product.short_name.as_str()),
            ("platform.id", self.platform.id.as_str()),
            ("platform.board", self.platform.board.as_str()),
            ("platform.architecture", self.platform.architecture.as_str()),
            (
                "platform.operatingSystem",
                self.platform.operating_system.as_str(),
            ),
        ] {
            if value.trim().is_empty() {
                return Err(CapabilityError::EmptyField(name));
            }
        }
        if self.video.status.is_available() && self.video.transports.is_empty() {
            return Err(CapabilityError::MissingVideoTransport);
        }
        if self
            .video
            .preferred_transport
            .is_some_and(|preferred| !self.video.transports.contains(&preferred))
        {
            return Err(CapabilityError::InvalidPreferredVideoTransport);
        }
        if self.hid.status.is_available() && self.hid.modes.is_empty() {
            return Err(CapabilityError::MissingHidMode);
        }
        if self.power.status.is_available() && self.power.actions.is_empty() {
            return Err(CapabilityError::MissingPowerAction);
        }
        if self.virtual_media.status.is_available() && self.virtual_media.modes.is_empty() {
            return Err(CapabilityError::MissingVirtualMediaMode);
        }
        if self.system.maintenance_write_mode && !self.system.read_only_rootfs {
            return Err(CapabilityError::InvalidMaintenanceWriteMode);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_manifest() -> CapabilityManifest {
        CapabilityManifest {
            schema_version: CAPABILITY_SCHEMA_VERSION,
            product: ProductIdentity {
                name: "BLi-KVM".into(),
                short_name: "BLi-KVM".into(),
                tagline: "Hardened with Rust".into(),
            },
            platform: PlatformIdentity {
                id: "test".into(),
                board: "test board".into(),
                architecture: "aarch64".into(),
                operating_system: "test OS".into(),
            },
            video: VideoCapabilities {
                status: FeatureStatus::unavailable(),
                transports: vec![],
                preferred_transport: None,
                max_width: None,
                max_height: None,
                audio: FeatureStatus::unavailable(),
            },
            hid: HidCapabilities {
                status: FeatureStatus::unavailable(),
                modes: vec![],
            },
            power: PowerCapabilities {
                status: FeatureStatus::unavailable(),
                actions: vec![],
                led_state: FeatureStatus::unavailable(),
            },
            virtual_media: VirtualMediaCapabilities {
                status: FeatureStatus::unavailable(),
                modes: vec![],
                resumable_upload: FeatureStatus::unavailable(),
                remote_download: FeatureStatus::unavailable(),
            },
            board: BoardCapabilities {
                features: BTreeMap::new(),
            },
            system: SystemCapabilities {
                status: FeatureStatus::unavailable(),
                read_only_rootfs: false,
                maintenance_write_mode: false,
                signed_application_updates: FeatureStatus::unavailable(),
                signed_system_updates: FeatureStatus::unavailable(),
                automatic_rollback: FeatureStatus::unavailable(),
            },
        }
    }

    #[test]
    fn minimal_manifest_is_valid() {
        assert_eq!(minimal_manifest().validate(), Ok(()));
    }

    #[test]
    fn hardware_evidence_does_not_imply_backend_exposure() {
        let status = FeatureStatus::not_exposed(EvidenceLevel::RuntimeVerified);
        assert!(!status.is_available());
        assert_eq!(status.evidence, EvidenceLevel::RuntimeVerified);
    }

    #[test]
    fn available_video_requires_transport() {
        let mut manifest = minimal_manifest();
        manifest.video.status = FeatureStatus::read_only(EvidenceLevel::SourceVerified);
        assert_eq!(
            manifest.validate(),
            Err(CapabilityError::MissingVideoTransport)
        );
    }

    #[test]
    fn preferred_transport_must_be_advertised() {
        let mut manifest = minimal_manifest();
        manifest.video.preferred_transport = Some(VideoTransport::Mjpeg);
        assert_eq!(
            manifest.validate(),
            Err(CapabilityError::InvalidPreferredVideoTransport)
        );
    }

    #[test]
    fn maintenance_mode_requires_read_only_rootfs() {
        let mut manifest = minimal_manifest();
        manifest.system.maintenance_write_mode = true;
        assert_eq!(
            manifest.validate(),
            Err(CapabilityError::InvalidMaintenanceWriteMode)
        );
    }

    #[test]
    fn serialized_contract_uses_camel_case_fields() {
        let value = serde_json::to_value(minimal_manifest()).unwrap();
        assert_eq!(value["schemaVersion"], CAPABILITY_SCHEMA_VERSION);
        assert_eq!(value["product"]["shortName"], "BLi-KVM");
        assert!(value.get("schema_version").is_none());
    }
}
