use std::error::Error;

use bli_kvm_control_broker::{ControlBrokerConfig, run};

#[cfg(feature = "hid-transition-profile")]
fn hid_profile_enabled() -> bool {
    std::env::var("BLI_KVM_HID_PROFILE").as_deref() == Ok("1")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    #[cfg(feature = "hid-transition-profile")]
    if let Some(profile) = bli_kvm_control_broker::hid_profile_from_values(
        hid_profile_enabled(),
        std::env::var("BLI_KVM_HID_LEDGER").ok().as_deref(),
        std::env::var("BLI_KVM_HID_KVMD_SOCKET").ok().as_deref(),
    )? {
        let directory = std::env::var_os("CREDENTIALS_DIRECTORY")
            .ok_or("HID transition profile requires CREDENTIALS_DIRECTORY")?;
        let credentials = bli_kvm_control_broker::kvmd_credentials_from_directory(
            std::path::Path::new(&directory),
        )?;
        bli_kvm_control_broker::run_with_hid_profile(
            ControlBrokerConfig::from_environment()?,
            profile.ledger,
            profile.kvmd_socket,
            credentials,
        )
        .await?;
        return Ok(());
    }

    run(ControlBrokerConfig::from_environment()?).await?;
    Ok(())
}
