//! Hardened BliKVM board daemon.
//!
//! One process owns the board peripherals the vendor C daemon used to drive.
//! The first slice covers the fan: the reviewed line, a soft-PWM actuator
//! behind the existing fan policy and supervisor, a thermal sampler and the
//! typed control surface. Everything is configured from the operator file and
//! nothing about the board is guessed.

pub mod config;
pub mod display;
pub mod fan;
pub mod server;
pub mod spi;

use std::{
    error::Error,
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use bli_kvm_fan_supervisor::FanSupervisor;
use std::sync::Arc;

use bli_kvm_platform_bli::{GpioRole, LineDirection, LineRequest};
use config::BoarddConfig;
use fan::SoftPwmFan;
use server::{BoarddState, FanModeName};

/// Consumer label the daemon requests its lines with.
pub const BOARD_DAEMON_CONSUMER: &str = "hardened-blikvm-boardd";

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Build the reviewed fan line request. The offset and polarity come from the
/// operator file; the role and consumer are the daemon's fixed identity.
pub fn fan_line(config: &BoarddConfig) -> LineRequest {
    LineRequest {
        chip: "gpiochip0",
        offset: config.fan.line,
        consumer: BOARD_DAEMON_CONSUMER,
        role: GpioRole::FanPwm,
        direction: LineDirection::Output,
        active_low: config.fan.active_low,
    }
}

/// Build the state for one board, optionally enabling real actuation.
pub fn build_state(
    config: &BoarddConfig,
    pwm_enabled: bool,
) -> Result<BoarddState<SoftPwmFan<bli_kvm_gpio_chardev::ChardevGpioDriver>>, Box<dyn Error>> {
    let driver = bli_kvm_gpio_chardev::ChardevGpioDriver::open(&config.gpio_chip)?;
    let mut fan = SoftPwmFan::new(
        driver,
        fan_line(config),
        config.fan.pwm_period_ms,
        pwm_enabled,
    );
    fan.start()?;
    let mut supervisor = FanSupervisor::new(config.fan.policy_config(), unix_millis(), fan)?;
    supervisor.initialize()?;
    Ok(BoarddState::new(
        supervisor,
        FanModeName::Auto,
        0,
        config.fan.minimum_duty_percent,
        config.fan.maximum_duty_percent,
        pwm_enabled,
        config.fan.pwm_period_ms,
        config.fan.line,
    ))
}

/// Run one automatic evaluation and print the status, then release the line.
///
/// This is the read-only shape of the fan gate: it never enables PWM, so it
/// verifies the plan, the executable, the policy wiring and the control surface
/// before the operator hands the line over.
pub fn run_once(config_path: &Path) -> Result<(), Box<dyn Error>> {
    let config = BoarddConfig::from_path(config_path)?;
    let mut state = build_state(&config, false)?;
    let thermal = bli_kvm_platform_bli::read_h616_temperature_millidegrees();
    state.note_thermal(thermal.map_err(|error| error.class()));
    state.sample(unix_millis())?;
    let view = state.status();
    println!("{}", serde_json::to_string(&view)?);
    Ok(())
}

/// Serve the control surface and sample the temperature until interrupted.
pub fn run_serve(config_path: &Path) -> Result<(), Box<dyn Error>> {
    let config = BoarddConfig::from_path(config_path)?;
    let state = Arc::new(Mutex::new(build_state(&config, true)?));
    let interval = std::time::Duration::from_millis(config.sample_interval_ms);
    let sampler_state = Arc::clone(&state);
    std::thread::spawn(move || {
        loop {
            let thermal = bli_kvm_platform_bli::read_h616_temperature_millidegrees();
            if let Ok(mut state) = sampler_state.lock() {
                state.note_thermal(thermal.map_err(|error| error.class()));
                let _ = state.sample(unix_millis());
            }
            std::thread::sleep(interval);
        }
    });
    server::serve(&config.socket, &state)?;
    Ok(())
}
