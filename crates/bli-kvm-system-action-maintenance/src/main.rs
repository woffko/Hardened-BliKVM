use std::{env, io::Write, process::ExitCode};

use bli_kvm_system_action_maintenance::{parse_invocation, run_exact_invocation};

fn main() -> ExitCode {
    match parse_invocation(env::args_os()).and_then(run_exact_invocation) {
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
            eprintln!("Hardened BliKVM system-action maintenance failed: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}
