use std::{io::Write, process::ExitCode};

use bli_kvm_diagnostic_collector::run;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let stderr = std::io::stderr();
            let mut locked = stderr.lock();
            let _ = writeln!(
                locked,
                "Hardened BliKVM diagnostic collector failed: {error}"
            );
            ExitCode::from(1)
        }
    }
}
