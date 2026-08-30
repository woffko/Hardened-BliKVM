use std::error::Error;

use bli_kvm_video_proxy::{VideoProxyConfig, run};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    run(VideoProxyConfig::from_environment()?).await?;
    Ok(())
}
