//! Hardened BliKVM board daemon.
//!
//! One process owns the board peripherals the vendor C daemon used to drive.
//! The first slice covers the fan: the reviewed line, a soft-PWM actuator
//! behind the existing fan policy and supervisor, a thermal sampler and the
//! typed control surface. Everything is configured from the operator file and
//! nothing about the board is guessed.

pub mod atx_leds;
pub mod button;
pub mod config;
pub mod display;
pub mod fan;
pub mod led;
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
    let display = build_display(config, pwm_enabled);
    let led = build_led(config, pwm_enabled);
    let button = build_button(config, pwm_enabled);
    let atx_leds = build_atx_leds(config, pwm_enabled);
    Ok(BoarddState::new(
        supervisor,
        FanModeName::Auto,
        0,
        config.fan.minimum_duty_percent,
        config.fan.maximum_duty_percent,
        pwm_enabled,
        config.fan.pwm_period_ms,
        config.fan.line,
        display,
        led,
        button,
        atx_leds,
    ))
}

/// Build the ATX host LED inputs; a failure never stops the fan.
fn build_atx_leds(
    config: &config::BoarddConfig,
    actuation_enabled: bool,
) -> Option<Box<dyn server::AtxLedControl>> {
    let atx_leds = config.atx_leds.as_ref()?;
    let _ = actuation_enabled;
    let driver = match bli_kvm_gpio_chardev::ChardevGpioDriver::open(&config.gpio_chip) {
        Ok(driver) => driver,
        Err(error) => {
            eprintln!("boardd: ATX LED lines unavailable: {error:?}");
            return None;
        }
    };
    match atx_leds::AtxLedsRuntime::new(
        driver,
        atx_leds::atx_power_led_line(atx_leds),
        atx_leds::atx_hdd_led_line(atx_leds),
        atx_leds,
    ) {
        Ok(runtime) => Some(Box::new(runtime)),
        Err(error) => {
            eprintln!("boardd: ATX LED inputs unavailable: {error}");
            None
        }
    }
}

/// Build the display button runtime; a failure never stops the fan.
fn build_button(
    config: &config::BoarddConfig,
    actuation_enabled: bool,
) -> Option<Box<dyn server::ButtonControl>> {
    if !actuation_enabled {
        return None;
    }
    let driver = match bli_kvm_gpio_chardev::ChardevGpioDriver::open(&config.gpio_chip) {
        Ok(driver) => driver,
        Err(error) => {
            eprintln!("boardd: display button line unavailable: {error:?}");
            return None;
        }
    };
    let initial = if config.button.active_high {
        button::ButtonLevel::High
    } else {
        button::ButtonLevel::Low
    };
    match button::ButtonRuntime::new(
        driver,
        button::button_line(&config.button),
        &config.button,
        unix_millis(),
        initial,
    ) {
        Ok(runtime) => Some(Box::new(runtime)),
        Err(error) => {
            eprintln!("boardd: display button unavailable: {error}");
            None
        }
    }
}

/// Build the activity LED runtime; a failure never stops the fan.
fn build_led(
    config: &config::BoarddConfig,
    actuation_enabled: bool,
) -> Option<Box<dyn server::LedControl>> {
    if !actuation_enabled {
        return None;
    }
    let driver = match bli_kvm_gpio_chardev::ChardevGpioDriver::open(&config.gpio_chip) {
        Ok(driver) => driver,
        Err(error) => {
            eprintln!("boardd: activity LED line unavailable: {error:?}");
            return None;
        }
    };
    let line = led::activity_led_line(&config.led);
    match led::LedRuntime::new(driver, line, &config.led, unix_millis()) {
        Ok(runtime) => Some(Box::new(runtime)),
        Err(error) => {
            eprintln!("boardd: activity LED unavailable: {error}");
            None
        }
    }
}

/// Build the panel runtime when the daemon may actuate.
///
/// A panel that cannot be brought up never stops the fan: the failure is
/// reported on stderr and the status view shows the display as absent.
fn build_display(
    config: &config::BoarddConfig,
    actuation_enabled: bool,
) -> Option<Box<dyn server::DisplayControl>> {
    if !actuation_enabled {
        return None;
    }
    let bus = match spi::SpidevBus::open(&config.display.spidev, config.display.clock_hz) {
        Ok(bus) => bus,
        Err(error) => {
            eprintln!("boardd: display bus unavailable: {error}");
            return None;
        }
    };
    let driver = match bli_kvm_gpio_chardev::ChardevGpioDriver::open(&config.gpio_chip) {
        Ok(driver) => driver,
        Err(error) => {
            eprintln!("boardd: display lines unavailable: {error:?}");
            return None;
        }
    };
    let mut display =
        display::Display::new(bus, driver, display::ThreadSleeper, config.display.clone());
    if let Err(error) = display.initialize() {
        eprintln!("boardd: display initialization failed: {error}");
        return None;
    }
    Some(Box::new(display))
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
    let rtc = bli_kvm_platform_bli::read_h616_external_rtc();
    state.note_external_rtc(
        rtc.map(|readout| (readout.unix_seconds, readout.offset_from_system_seconds))
            .map_err(|error| error.class()),
    );
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
            let rtc = bli_kvm_platform_bli::read_h616_external_rtc();
            if let Ok(mut state) = sampler_state.lock() {
                state.note_thermal(thermal.map_err(|error| error.class()));
                state.note_external_rtc(
                    rtc.map(|readout| (readout.unix_seconds, readout.offset_from_system_seconds))
                        .map_err(|error| error.class()),
                );
                let _ = state.sample(unix_millis());
                state.tick_led();
                state.tick_button();
                state.tick_atx_leds();
            }
            std::thread::sleep(interval);
        }
    });
    server::serve(&config.socket, &state)?;
    Ok(())
}
