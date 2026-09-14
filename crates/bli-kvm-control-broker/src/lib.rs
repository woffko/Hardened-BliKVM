mod attestation;
mod config;
#[cfg(any(
    feature = "hid-transition-profile",
    feature = "atx-transition-profile",
    feature = "media-transition-profile",
    feature = "gadget-media-profile",
    feature = "gadget-hid-profile",
    feature = "gpio-atx-profile",
    feature = "chardev-atx-profile"
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
    feature = "gadget-hid-profile",
    feature = "gpio-atx-profile",
    feature = "chardev-atx-profile"
))]
pub use profile::ProfileError;
#[cfg(any(
    feature = "atx-transition-profile",
    feature = "gpio-atx-profile",
    feature = "chardev-atx-profile"
))]
pub use profile::atx_reconciliations_from_values;
#[cfg(any(
    feature = "hid-transition-profile",
    feature = "atx-transition-profile",
    feature = "media-transition-profile"
))]
pub use profile::kvmd_credentials_from_directory;
#[cfg(feature = "atx-transition-profile")]
pub use profile::{AtxProfile, atx_profile_from_values};
#[cfg(feature = "gadget-media-profile")]
pub use profile::{
    GADGET_BCD_DEVICE, GADGET_BCD_USB, GADGET_CONFIGURATION, GADGET_ID_PRODUCT, GADGET_ID_VENDOR,
    GADGET_MANUFACTURER, GADGET_PRODUCT, GadgetMediaProfile, gadget_media_config,
    gadget_media_profile_from_values, gadget_serial_from_file,
};
#[cfg(feature = "gadget-hid-profile")]
pub use profile::{GadgetHidProfile, gadget_hid_config, gadget_hid_profile_from_values};
#[cfg(feature = "hid-transition-profile")]
pub use profile::{HidProfile, hid_profile_from_values};
#[cfg(feature = "media-transition-profile")]
pub use profile::{MediaProfile, media_profile_from_values};
#[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
pub use server::AtxReconciliation;
#[cfg(feature = "atx-transition-profile")]
pub use server::run_with_atx_profile;
#[cfg(all(
    feature = "gadget-media-profile",
    feature = "gadget-hid-profile",
    feature = "gpio-atx-profile"
))]
pub use server::run_with_board_profile;
#[cfg(feature = "chardev-atx-profile")]
pub use server::run_with_chardev_atx_profile;
#[cfg(all(
    feature = "gadget-media-profile",
    feature = "gadget-hid-profile",
    feature = "chardev-atx-profile"
))]
pub use server::run_with_chardev_board_profile;
#[cfg(all(feature = "hid-transition-profile", feature = "atx-transition-profile"))]
pub use server::run_with_control_profile;
#[cfg(feature = "gadget-hid-profile")]
pub use server::run_with_gadget_hid_profile;
#[cfg(feature = "gadget-media-profile")]
pub use server::run_with_gadget_media_profile;
#[cfg(all(feature = "gadget-media-profile", feature = "gadget-hid-profile"))]
pub use server::run_with_gadget_profile;
#[cfg(feature = "gpio-atx-profile")]
pub use server::run_with_gpio_atx_profile;
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
pub use server::{ControlBrokerError, run};
pub use v2::ControlV2State;
