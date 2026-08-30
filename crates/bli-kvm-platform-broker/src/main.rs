use std::error::Error;

use bli_kvm_platform_broker::{BrokerConfig, load_systemd_credentials, run};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = BrokerConfig::from_systemd_environment()?;
    let credentials = load_systemd_credentials(&config.credential_directory)?;
    run(config, credentials).await?;
    Ok(())
}
