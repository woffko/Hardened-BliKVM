mod admin_features;
mod attestation;
pub mod board_status;
mod config;
mod device_settings;
pub mod firewall_confirmation;
pub mod firewall_executor;
pub mod firewall_health;
pub mod firewall_observation;
mod firewall_process;
pub mod firewall_rules;
pub mod firewall_runtime;
pub mod firewall_settings;
pub mod firewall_transaction;
mod hostname_settings;
mod ipv4_settings;
pub mod mouse_jiggler;
pub mod network_settings;
#[cfg(feature = "network-write-profile")]
pub mod network_write;
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
mod ssh_settings;
#[cfg(feature = "system-action-profile")]
pub mod system_action;
pub mod system_log_settings;
pub mod tailscale;
pub mod tls_settings;
mod video_settings;
pub use device_settings::{HID_ONLY_STATE, HidOnlyState, MDNS_CONF, MDNS_UNIT, MdnsState};
pub use firewall_settings::{
    FIREWALL_STATE_DIR, FirewallRunner, FirewallState, FirewallView, SystemFirewallRunner,
};
pub use mouse_jiggler::{JiggleWriter, MouseJigglerState};
pub use network_settings::{
    NM_CONNECTIONS_DIR, NmcliRunner, SharedWifi, WifiRunner, WifiState, WifiStatus,
};
pub use tls_settings::{
    CertificateView, NGINX_CONF, NGINX_UNIT, SystemTlsRunner, TlsRunner, TlsState,
};
pub use video_settings::{ExecVideoRestartRunner, VideoRestartRunner, VideoSettingsState};
mod v2;

pub use attestation::{
    DEFAULT_MAX_ATTESTATIONS, TargetAttestationError, TargetAttestationRegistry,
};
pub use config::{ControlBrokerConfig, ControlBrokerConfigError};

/// Origin boot binding for the system-action policy: the running boot's
/// identity, so a request prepared before a reboot cannot be replayed after it.
pub fn boot_binding_from_system() -> Result<[u8; 16], Box<dyn std::error::Error>> {
    let raw = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    let hex: String = raw
        .chars()
        .filter(|character| character.is_ascii_hexdigit())
        .collect();
    if hex.len() < 32 {
        return Err("boot id is too short".into());
    }
    let mut binding = [0_u8; 16];
    for (index, byte) in binding.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)?;
    }
    Ok(binding)
}
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
#[cfg(all(
    feature = "gadget-media-profile",
    feature = "gadget-hid-profile",
    feature = "chardev-atx-profile",
    feature = "system-action-profile"
))]
pub use server::run_with_chardev_board_profile_with_system_action;
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
