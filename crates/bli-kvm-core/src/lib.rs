//! Platform-neutral contracts for BLi-KVM.
//!
//! Hardware access belongs in platform adapters and the future privileged
//! broker. This crate intentionally contains no device paths, shell commands,
//! network listeners, or credentials.

mod capabilities;
mod control;
mod control_v2;
mod diagnostics;
mod inventory;
mod platform;
mod video_settings;

pub use capabilities::{
    Availability, BoardCapabilities, CapabilityError, CapabilityManifest, EvidenceLevel,
    FeatureStatus, HidCapabilities, HidMode, NetworkCapabilities, PlatformIdentity, PowerAction,
    PowerCapabilities, ProductIdentity, SystemCapabilities, VideoCapabilities, VideoSetting,
    VideoTransport, VirtualMediaCapabilities, VirtualMediaMode,
};
pub use control::{
    AtxControlAction, ControlValidationError, HidReport, PlatformControlRequest,
    VirtualMediaCommand, VirtualMediaControlMode,
};
pub use control_v2::{
    AcceptPostBootRequest, AtxConfirmation, ControlV2ValidationError, HID_LEASE_MAX_TTL_SECONDS,
    HidLeaseRequest, MediaObserveRequest, MediaRequest, NetworkWriteRequest, NetworkWriteTarget,
    PlatformControlRequestV2, SystemActionKind, SystemActionWireRequest,
    TARGET_ATTESTATION_MAX_TTL_SECONDS, TARGET_ATTESTATION_MIN_TTL_SECONDS, TargetAttestation,
    TargetRelease,
};
pub use diagnostics::{
    DIAGNOSTICS_VIEW_SCHEMA_VERSION, DiagnosticBooleanState, DiagnosticComponentHealth,
    DiagnosticComponents, DiagnosticHistory, DiagnosticHistoryReason, DiagnosticHistoryState,
    DiagnosticMetricState, DiagnosticRtc, DiagnosticTemperature, DiagnosticsAvailability,
    DiagnosticsCurrent, DiagnosticsView, DiagnosticsViewError,
};
pub use inventory::{
    ComponentObservation, InventoryBackend, InventoryError, RUNTIME_INVENTORY_SCHEMA_VERSION,
    RtcObservation, RuntimeInventory, RuntimeVersions, ThermalObservation,
};
pub use platform::CapabilityProvider;
pub use video_settings::{
    JIGGLE_DELTA, JIGGLE_INTERVAL_SECONDS, MouseJigglerChange, MouseJigglerError, MouseJigglerMode,
    VIDEO_RESOLUTION_DEFAULT, VIDEO_RESOLUTION_ENV, VIDEO_RESOLUTIONS, jiggle_reports,
    resolution_for_height, reviewed_resolution,
};
pub use video_settings::{
    VIDEO_FPS_DEFAULT, VIDEO_FPS_ENV, VIDEO_FPS_MAX, VIDEO_FPS_MIN, VIDEO_QUALITY_DEFAULT,
    VIDEO_QUALITY_ENV, VIDEO_QUALITY_MAX, VIDEO_QUALITY_MIN, VideoSettings, VideoSettingsChange,
    VideoSettingsError,
};

/// Version of the JSON capability contract consumed by the web interface.
pub const CAPABILITY_SCHEMA_VERSION: u16 = 1;
