//! Platform-neutral contracts for BLi-KVM.
//!
//! Hardware access belongs in platform adapters and the future privileged
//! broker. This crate intentionally contains no device paths, shell commands,
//! network listeners, or credentials.

mod capabilities;
mod control;
mod inventory;
mod platform;

pub use capabilities::{
    Availability, BoardCapabilities, CapabilityError, CapabilityManifest, EvidenceLevel,
    FeatureStatus, HidCapabilities, HidMode, PlatformIdentity, PowerAction, PowerCapabilities,
    ProductIdentity, SystemCapabilities, VideoCapabilities, VideoTransport,
    VirtualMediaCapabilities, VirtualMediaMode,
};
pub use control::{
    AtxControlAction, ControlValidationError, HidReport, PlatformControlRequest,
    VirtualMediaCommand, VirtualMediaControlMode,
};
pub use inventory::{
    ComponentObservation, InventoryBackend, InventoryError, RUNTIME_INVENTORY_SCHEMA_VERSION,
    RuntimeInventory, RuntimeVersions,
};
pub use platform::CapabilityProvider;

/// Version of the JSON capability contract consumed by the web interface.
pub const CAPABILITY_SCHEMA_VERSION: u16 = 1;
