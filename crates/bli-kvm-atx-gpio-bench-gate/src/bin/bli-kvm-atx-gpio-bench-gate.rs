//! Argument-free entry point for the native ATX GPIO bench.
//!
//! Configuration arrives through the environment and the physical-disconnect
//! confirmation is mandatory; without it the bench refuses and exits non-zero.

use std::path::PathBuf;
use std::process::ExitCode;

use bli_kvm_atx_gpio_bench_gate::{BenchError, run_bench};
use bli_kvm_atx_gpio_executor::AtxTiming;
use bli_kvm_platform_bli::{H616_ATX_POWER_LONG_MS, H616_ATX_POWER_SHORT_MS, H616_ATX_RESET_MS};

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn main() -> ExitCode {
    if std::env::var("BLI_KVM_ATX_BENCH_CONFIRM").as_deref() != Ok("1") {
        eprintln!("atx-gpio-bench=not-confirmed");
        return ExitCode::FAILURE;
    }
    let chip = PathBuf::from(
        std::env::var("BLI_KVM_ATX_CHIP").unwrap_or_else(|_| "/dev/gpiochip0".to_owned()),
    );
    let timing = AtxTiming {
        power_short_ms: env_u64("BLI_KVM_ATX_POWER_SHORT_MS", H616_ATX_POWER_SHORT_MS),
        power_long_ms: env_u64("BLI_KVM_ATX_POWER_LONG_MS", H616_ATX_POWER_LONG_MS),
        reset_ms: env_u64("BLI_KVM_ATX_RESET_MS", H616_ATX_RESET_MS),
    };
    match run_bench(&chip, timing, true) {
        Ok(facts) => {
            let mut drift = false;
            for (key, value) in &facts {
                println!("{key}={value}");
                if key.ends_with("_within_tolerance") && value != "yes" {
                    drift = true;
                }
            }
            if drift {
                eprintln!("atx-gpio-bench=drift");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(BenchError::NotConfirmed) => {
            eprintln!("atx-gpio-bench=not-confirmed");
            ExitCode::FAILURE
        }
        Err(BenchError::Backend(message)) => {
            eprintln!("atx-gpio-bench=failed: {message}");
            ExitCode::FAILURE
        }
    }
}
