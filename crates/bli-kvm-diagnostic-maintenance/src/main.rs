use std::{env, io::Write, process::ExitCode};

use bli_kvm_diagnostic_maintenance::{parse_arguments, run_exact};

fn main() -> ExitCode {
    match parse_arguments(env::args_os()).and_then(run_exact) {
        Ok(receipt) => {
            let stdout = std::io::stdout();
            let mut locked = stdout.lock();
            if writeln!(locked, "{receipt}").is_ok() && locked.flush().is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(50)
            }
        }
        Err(error) => {
            eprintln!("Hardened BliKVM diagnostic maintenance failed: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}
