//! Argument-free entry point for the isolated virtual-media gate.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match bli_kvm_media_isolated_gate::run_isolated_checks().await {
        Ok(facts) => {
            for (key, value) in facts {
                println!("{key}={value}");
            }
            ExitCode::SUCCESS
        }
        Err(_) => {
            eprintln!("isolated-media-gate=failed");
            ExitCode::FAILURE
        }
    }
}
