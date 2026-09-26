//! `bli-kvm-system-update`: the boot-time and first-boot steps of the
//! RAW system update (`docs/design/system-raw-update-v1.md`).
//!
//! - `boot --staging DIR --device DEV` runs from the initramfs before the root
//!   is mounted. Exit status 0: continue booting; 10: the root partition
//!   changed, reboot; 1: the step failed and changed nothing it could not
//!   undo, continue booting.
//! - `booted [--staging DIR] [--health-url URL] [--timeout SECONDS]` runs on
//!   the new root once its services are up and marks the update booted after
//!   the web server answers its health check.
//! - `status [--staging DIR]` prints the apply record.
//!
//! Staging, confirming, rolling back and cancelling go through the
//! administration helper, which checks the operator's switch.

use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use bli_kvm_system_raw_update::{
    BootAction, Phase, SystemTools, boot, mark_booted, read_record, staging_path,
};

const REBOOT: u8 = 10;
const DEFAULT_HEALTH_URL: &str = "https://127.0.0.1/api/health";

struct Options {
    staging: PathBuf,
    device: Option<PathBuf>,
    health_url: String,
    timeout: Duration,
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: bli-kvm-system-update boot --staging DIR --device DEV\n       bli-kvm-system-update booted [--staging DIR] [--health-url URL] [--timeout SECONDS]\n       bli-kvm-system-update status [--staging DIR]"
    );
    ExitCode::from(2)
}

fn parse(mut arguments: impl Iterator<Item = String>) -> Option<Options> {
    let mut options = Options {
        staging: staging_path(std::path::Path::new("/mnt")),
        device: None,
        health_url: DEFAULT_HEALTH_URL.into(),
        timeout: Duration::from_secs(300),
    };
    while let Some(flag) = arguments.next() {
        let value = arguments.next()?;
        match flag.as_str() {
            "--staging" => options.staging = PathBuf::from(value),
            "--device" => options.device = Some(PathBuf::from(value)),
            "--health-url" => options.health_url = value,
            "--timeout" => options.timeout = Duration::from_secs(value.parse().ok()?),
            _ => return None,
        }
    }
    Some(options)
}

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let command = arguments.next();
    let Some(options) = parse(arguments) else {
        return usage();
    };
    match command.as_deref() {
        Some("boot") => boot_step(&options),
        Some("booted") => booted_step(&options),
        Some("status") => match read_record(&options.staging) {
            Ok(record) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&record).unwrap_or_default()
                );
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("system update: {error}");
                ExitCode::from(1)
            }
        },
        _ => usage(),
    }
}

fn boot_step(options: &Options) -> ExitCode {
    let Some(device) = options.device.as_deref() else {
        return usage();
    };
    match boot(&options.staging, device, &mut SystemTools) {
        Ok(action) => {
            if let Ok(Some(record)) = read_record(&options.staging)
                && !record.message.is_empty()
            {
                eprintln!("system update: {}", record.message);
            }
            match action {
                BootAction::Continue => ExitCode::SUCCESS,
                BootAction::Reboot => ExitCode::from(REBOOT),
            }
        }
        Err(error) => {
            eprintln!("system update: {error}");
            ExitCode::from(1)
        }
    }
}

fn healthy(url: &str) -> bool {
    // The device's own certificate is self-signed; the check only asks
    // whether the local web stack answers.
    Command::new("curl")
        .args([
            "--silent",
            "--fail",
            "--insecure",
            "--max-time",
            "5",
            "--output",
        ])
        .arg("/dev/null")
        .arg("--")
        .arg(url)
        .stdin(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn booted_step(options: &Options) -> ExitCode {
    match read_record(&options.staging) {
        Ok(Some(record)) if record.phase == Phase::Written => {}
        Ok(_) => return ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("system update: {error}");
            return ExitCode::from(1);
        }
    }
    let started = Instant::now();
    while !healthy(&options.health_url) {
        if started.elapsed() >= options.timeout {
            eprintln!("system update: the web server did not answer; the update stays unconfirmed");
            return ExitCode::from(1);
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    match mark_booted(&options.staging) {
        Ok(record) => {
            eprintln!(
                "system update: {} (sequence {}) booted",
                record.version, record.sequence
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("system update: {error}");
            ExitCode::from(1)
        }
    }
}
