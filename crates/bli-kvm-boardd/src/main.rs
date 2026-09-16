//! Board daemon entry point.

use std::{env, path::PathBuf, process::ExitCode};

use bli_kvm_boardd::{run_once, run_serve};

fn usage() -> ExitCode {
    eprintln!("usage: bli-kvm-boardd --config <path> [--once]");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut config: Option<PathBuf> = None;
    let mut once = false;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--config" => config = arguments.next().map(PathBuf::from),
            "--once" => once = true,
            _ => return usage(),
        }
    }
    let Some(config) = config else {
        return usage();
    };
    let result = if once {
        run_once(&config)
    } else {
        run_serve(&config)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bli-kvm-boardd: {error}");
            ExitCode::FAILURE
        }
    }
}
