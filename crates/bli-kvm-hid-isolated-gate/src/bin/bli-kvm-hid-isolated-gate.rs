//! Argument-free entry point for the isolated-backend HID transition gate.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match bli_kvm_hid_isolated_gate::run_isolated_checks().await {
        Ok(facts) => {
            for (key, value) in facts {
                println!("{key}={value}");
            }
            ExitCode::SUCCESS
        }
        Err(_) => {
            eprintln!("isolated-gate=failed");
            ExitCode::FAILURE
        }
    }
}
