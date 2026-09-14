mod attestation;
mod config;
#[cfg(any(
    feature = "hid-transition-profile",
    feature = "atx-transition-profile",
    feature = "media-transition-profile",
    feature = "gadget-media-profile",
    feature = "gadget-hid-profile"
))]
mod profile;
mod server;
mod v2;

pub use attestation::{
    DEFAULT_MAX_ATTESTATIONS, TargetAttestationError, TargetAttestationRegistry,
};
pub use config::{ControlBrokerConfig, ControlBrokerConfigError};
#[cfg(any(
    feature = "hid-transition-profile",
    feature = "atx-transition-profile",
    feature = "media-transition-profile",
    feature = "gadget-media-profile",
    feature = "gadget-hid-profile"
))]
pub use profile::ProfileError;
#[cfg(any(
    feature = "hid-transition-profile",
    feature = "atx-transition-profile",
    feature = "media-transition-profile"
))]
pub use profile::kvmd_credentials_from_directory;
#[cfg(feature = "atx-transition-profile")]
pub use profile::{AtxProfile, atx_profile_from_values, atx_reconciliations_from_values};
#[cfg(feature = "gadget-media-profile")]
pub use profile::{
    GADGET_BCD_DEVICE, GADGET_BCD_USB, GADGET_CONFIGURATION, GADGET_ID_PRODUCT, GADGET_ID_VENDOR,
    GADGET_MANUFACTURER, GADGET_PRODUCT, GadgetMediaProfile, gadget_media_config,
    gadget_media_profile_from_values,
};
#[cfg(feature = "gadget-hid-profile")]
pub use profile::{GadgetHidProfile, gadget_hid_config, gadget_hid_profile_from_values};
#[cfg(feature = "hid-transition-profile")]
pub use profile::{HidProfile, hid_profile_from_values};
#[cfg(feature = "media-transition-profile")]
pub use profile::{MediaProfile, media_profile_from_values};
#[cfg(all(feature = "hid-transition-profile", feature = "atx-transition-profile"))]
pub use server::run_with_control_profile;
#[cfg(feature = "gadget-hid-profile")]
pub use server::run_with_gadget_hid_profile;
#[cfg(feature = "gadget-media-profile")]
pub use server::run_with_gadget_media_profile;
#[cfg(feature = "hid-transition-profile")]
pub use server::run_with_hid_profile;
#[cfg(feature = "media-transition-profile")]
pub use server::run_with_media_profile;
#[cfg(all(
    feature = "hid-transition-profile",
    feature = "atx-transition-profile",
    feature = "media-transition-profile"
))]
pub use server::run_with_restricted_profiles;
#[cfg(feature = "atx-transition-profile")]
pub use server::{AtxReconciliation, run_with_atx_profile};
pub use server::{ControlBrokerError, run};
pub use v2::ControlV2State;
