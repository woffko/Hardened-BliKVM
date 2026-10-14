//! Platform-neutral contracts for BLi-KVM.
//!
//! Hardware access belongs in platform adapters and the future privileged
//! broker. This crate intentionally contains no device paths, shell commands,
//! network listeners, or credentials.

mod admin_features;
mod capabilities;
mod control;
mod control_v2;
mod diagnostics;
mod display_sleep;
mod firewall;
mod inventory;
mod platform;
mod video_settings;

pub use admin_features::{ADMIN_FEATURES_PATH, AdminFeatureSwitches};
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
    AcceptPostBootRequest, AdminFeature, AtxConfirmation, ControlV2ValidationError,
    FirewallRulesFamily, HID_LEASE_MAX_TTL_SECONDS, HidLeaseRequest, Ipv4Change, Ipv6Change,
    MAX_FIREWALL_RULES_BYTES, MAX_FIREWALL_RULES_CHUNK, MAX_IPV4_DNS_SERVERS,
    MAX_TARGET_AGENT_CALL_MS, MAX_WIFI_NETWORKS, MAX_WIFI_PROFILES, MediaObserveRequest,
    MediaRequest, NetworkWriteRequest, NetworkWriteTarget, PlatformControlRequestV2,
    SystemActionKind, SystemActionWireRequest, TARGET_ATTESTATION_MAX_TTL_SECONDS,
    TARGET_ATTESTATION_MIN_TTL_SECONDS, TargetAgentLease, TargetAttestation, TargetRelease,
    WifiProfileAction, valid_hostname, valid_wifi_password, valid_wifi_profile_id, valid_wifi_ssid,
};
pub use diagnostics::{
    DIAGNOSTICS_VIEW_SCHEMA_VERSION, DiagnosticBooleanState, DiagnosticComponentHealth,
    DiagnosticComponents, DiagnosticHistory, DiagnosticHistoryReason, DiagnosticHistoryState,
    DiagnosticMetricState, DiagnosticRtc, DiagnosticTemperature, DiagnosticsAvailability,
    DiagnosticsCurrent, DiagnosticsView, DiagnosticsViewError,
};
pub use display_sleep::{
    DISPLAY_SLEEP_CHOICES, DISPLAY_SLEEP_PATH, display_sleep_is_choice, load_display_sleep,
    parse_display_sleep, render_display_sleep,
};
pub use firewall::{
    FirewallChangeRequest, FirewallConfirmRequest, FirewallMode, FirewallModeStatus,
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
