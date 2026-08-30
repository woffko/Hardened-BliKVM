use std::error::Error;

use bli_kvm_control_broker::{ControlBrokerConfig, run};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    run(ControlBrokerConfig::from_environment()?).await?;
    Ok(())
}
