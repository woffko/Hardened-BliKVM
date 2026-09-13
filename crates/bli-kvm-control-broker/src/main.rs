use std::error::Error;

use bli_kvm_control_broker::{ControlBrokerConfig, run};

#[cfg(feature = "hid-transition-profile")]
fn hid_profile_enabled() -> bool {
    std::env::var("BLI_KVM_HID_PROFILE").as_deref() == Ok("1")
}

#[cfg(feature = "atx-transition-profile")]
fn atx_profile_enabled() -> bool {
    std::env::var("BLI_KVM_ATX_PROFILE").as_deref() == Ok("1")
}

#[cfg(feature = "media-transition-profile")]
fn media_profile_enabled() -> bool {
    std::env::var("BLI_KVM_MEDIA_PROFILE").as_deref() == Ok("1")
}

#[cfg(any(
    feature = "hid-transition-profile",
    feature = "atx-transition-profile",
    feature = "media-transition-profile"
))]
fn credentials_directory() -> Result<std::path::PathBuf, Box<dyn Error>> {
    std::env::var_os("CREDENTIALS_DIRECTORY")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| "transition profile requires CREDENTIALS_DIRECTORY".into())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    #[cfg(all(feature = "hid-transition-profile", feature = "atx-transition-profile"))]
    {
        let hid = bli_kvm_control_broker::hid_profile_from_values(
            hid_profile_enabled(),
            std::env::var("BLI_KVM_HID_LEDGER").ok().as_deref(),
            std::env::var("BLI_KVM_HID_KVMD_SOCKET").ok().as_deref(),
        )?;
        let atx = bli_kvm_control_broker::atx_profile_from_values(
            atx_profile_enabled(),
            std::env::var("BLI_KVM_ATX_JOURNAL").ok().as_deref(),
            std::env::var("BLI_KVM_ATX_KVMD_SOCKET").ok().as_deref(),
        )?;
        match (hid, atx) {
            (Some(hid), Some(atx)) => {
                let directory = credentials_directory()?;
                let reconciliations = bli_kvm_control_broker::atx_reconciliations_from_values(
                    atx_profile_enabled(),
                    std::env::var("BLI_KVM_ATX_RECONCILIATIONS").ok().as_deref(),
                )?;
                bli_kvm_control_broker::run_with_control_profile(
                    ControlBrokerConfig::from_environment()?,
                    hid.ledger,
                    hid.kvmd_socket,
                    atx.journal,
                    atx.kvmd_socket,
                    directory,
                    reconciliations,
                )
                .await?;
                return Ok(());
            }
            (Some(hid), None) => {
                let directory = credentials_directory()?;
                let credentials =
                    bli_kvm_control_broker::kvmd_credentials_from_directory(&directory)?;
                bli_kvm_control_broker::run_with_hid_profile(
                    ControlBrokerConfig::from_environment()?,
                    hid.ledger,
                    hid.kvmd_socket,
                    credentials,
                )
                .await?;
                return Ok(());
            }
            (None, Some(atx)) => {
                let directory = credentials_directory()?;
                let credentials =
                    bli_kvm_control_broker::kvmd_credentials_from_directory(&directory)?;
                let reconciliations = bli_kvm_control_broker::atx_reconciliations_from_values(
                    true,
                    std::env::var("BLI_KVM_ATX_RECONCILIATIONS").ok().as_deref(),
                )?;
                bli_kvm_control_broker::run_with_atx_profile(
                    ControlBrokerConfig::from_environment()?,
                    atx.journal,
                    atx.kvmd_socket,
                    credentials,
                    reconciliations,
                )
                .await?;
                return Ok(());
            }
            (None, None) => {}
        }
    }

    #[cfg(all(
        feature = "hid-transition-profile",
        not(feature = "atx-transition-profile")
    ))]
    if let Some(profile) = bli_kvm_control_broker::hid_profile_from_values(
        hid_profile_enabled(),
        std::env::var("BLI_KVM_HID_LEDGER").ok().as_deref(),
        std::env::var("BLI_KVM_HID_KVMD_SOCKET").ok().as_deref(),
    )? {
        let credentials =
            bli_kvm_control_broker::kvmd_credentials_from_directory(&credentials_directory()?)?;
        bli_kvm_control_broker::run_with_hid_profile(
            ControlBrokerConfig::from_environment()?,
            profile.ledger,
            profile.kvmd_socket,
            credentials,
        )
        .await?;
        return Ok(());
    }

    #[cfg(all(
        feature = "atx-transition-profile",
        not(feature = "hid-transition-profile")
    ))]
    if let Some(profile) = bli_kvm_control_broker::atx_profile_from_values(
        atx_profile_enabled(),
        std::env::var("BLI_KVM_ATX_JOURNAL").ok().as_deref(),
        std::env::var("BLI_KVM_ATX_KVMD_SOCKET").ok().as_deref(),
    )? {
        let credentials =
            bli_kvm_control_broker::kvmd_credentials_from_directory(&credentials_directory()?)?;
        let reconciliations = bli_kvm_control_broker::atx_reconciliations_from_values(
            true,
            std::env::var("BLI_KVM_ATX_RECONCILIATIONS").ok().as_deref(),
        )?;
        bli_kvm_control_broker::run_with_atx_profile(
            ControlBrokerConfig::from_environment()?,
            profile.journal,
            profile.kvmd_socket,
            credentials,
            reconciliations,
        )
        .await?;
        return Ok(());
    }

    #[cfg(feature = "media-transition-profile")]
    if let Some(profile) = bli_kvm_control_broker::media_profile_from_values(
        media_profile_enabled(),
        std::env::var("BLI_KVM_MEDIA_JOURNAL").ok().as_deref(),
        std::env::var("BLI_KVM_MEDIA_CATALOG").ok().as_deref(),
        std::env::var("BLI_KVM_MEDIA_KVMD_SOCKET").ok().as_deref(),
    )? {
        let credentials =
            bli_kvm_control_broker::kvmd_credentials_from_directory(&credentials_directory()?)?;
        bli_kvm_control_broker::run_with_media_profile(
            ControlBrokerConfig::from_environment()?,
            profile.journal,
            profile.catalog,
            profile.kvmd_socket,
            credentials,
        )
        .await?;
        return Ok(());
    }

    run(ControlBrokerConfig::from_environment()?).await?;
    Ok(())
}
