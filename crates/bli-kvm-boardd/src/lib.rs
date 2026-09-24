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
pub mod panel_font;
pub mod panel_status;
mod sampler;
pub mod server;
pub mod spi;

use std::{
    error::Error,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
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
    let mut state = BoarddState::new(
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
    );
    state.configure_display_status(config.display.status_interval_ms);
    Ok(state)
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
    run_serve_until(config_path, Arc::new(AtomicBool::new(false)))
}

/// Stop admission and join sampling before the final fan failsafe write.
/// Device I/O and mutex acquisition still require the external service deadline.
pub fn run_serve_until(
    config_path: &Path,
    stopping: Arc<AtomicBool>,
) -> Result<(), Box<dyn Error>> {
    let config = BoarddConfig::from_path(config_path)?;
    if stopping.load(Ordering::SeqCst) {
        return Ok(());
    }
    let state = Arc::new(Mutex::new(build_state(&config, true)?));
    if let Ok(mut board) = state.lock() {
        // Draw a defined frame immediately: an ST7789 keeps undefined GRAM
        // after reset until the first full frame, which otherwise shows garbage.
        let _ = board.draw_display_status();
    }
    let interval = std::time::Duration::from_millis(config.sample_interval_ms);
    let sampler_state = Arc::clone(&state);
    let sample_stop = Arc::clone(&stopping);
    let serving = (|| -> Result<(), Box<dyn Error>> {
        let sampler = sampler::Sampler::spawn(Arc::clone(&stopping), interval, move || {
            let thermal = bli_kvm_platform_bli::read_h616_temperature_millidegrees();
            let rtc = bli_kvm_platform_bli::read_h616_external_rtc();
            {
                let mut state = sampler_state
                    .lock()
                    .map_err(|_| std::io::Error::other("boardd state poisoned"))?;
                if sample_stop.load(Ordering::SeqCst) {
                    return Ok(());
                }
                state.note_thermal(thermal.map_err(|error| error.class()));
                state.note_external_rtc(
                    rtc.map(|readout| (readout.unix_seconds, readout.offset_from_system_seconds))
                        .map_err(|error| error.class()),
                );
                let _ = state.sample(unix_millis());
                // A failed background actuator is fatal in either fan mode.
                // Stop admission rather than serving stale supervisor health.
                state
                    .ensure_fan_runtime()
                    .map_err(|_| std::io::Error::other("boardd fan runtime failed"))?;
                state.tick_led();
                state.tick_button();
                state.tick_atx_leds();
                state.tick_display(unix_millis());
            }
            Ok(())
        })?;
        let listener =
            server::serve_until(&config.socket, &state, || stopping.load(Ordering::SeqCst));
        let joined = sampler.finish();
        // Evaluate both outcomes: a listener error must still stop/join sampling.
        listener?;
        joined?;
        Ok(())
    })();
    let cleanup = cleanup_state(&state, |board| {
        board.prepare_shutdown()?.quiesce_at_failsafe()?;
        Ok(())
    });
    match (serving, cleanup) {
        (Ok(()), result) | (result, Ok(())) => result,
        (Err(serving), Err(cleanup)) => {
            Err(std::io::Error::other(format!("{serving}; shutdown failed: {cleanup}")).into())
        }
    }
}

/// Poisoning invalidates normal control, but must not skip the failsafe attempt.
/// Even successful cleanup cannot turn a poisoned runtime into a clean stop.
fn cleanup_state<S>(
    state: &Mutex<S>,
    cleanup: impl FnOnce(&mut S) -> Result<(), Box<dyn Error>>,
) -> Result<(), Box<dyn Error>> {
    let (mut state, poisoned) = match state.lock() {
        Ok(state) => (state, false),
        Err(error) => (error.into_inner(), true),
    };
    let result = cleanup(&mut state);
    if poisoned {
        let message = match result {
            Ok(()) => "boardd state poisoned during shutdown; failsafe cleanup succeeded".into(),
            Err(error) => {
                format!("boardd state poisoned during shutdown; failsafe cleanup failed: {error}")
            }
        };
        Err(std::io::Error::other(message).into())
    } else {
        result
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;

    #[test]
    fn poisoned_state_still_attempts_cleanup_and_never_reports_clean() {
        for fail_cleanup in [false, true] {
            let state = Arc::new(Mutex::new(false));
            let worker_state = state.clone();
            assert!(
                std::thread::spawn(move || {
                    let _state = worker_state.lock().unwrap();
                    panic!("fixture state mutation panic");
                })
                .join()
                .is_err()
            );
            let error = cleanup_state(&state, |attempted| {
                *attempted = true;
                if fail_cleanup {
                    Err(std::io::Error::other("fixture GPIO failure").into())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            assert!(*state.lock().unwrap_err().into_inner());
            assert!(error.to_string().contains("state poisoned"));
            assert_eq!(
                error.to_string().contains("fixture GPIO failure"),
                fail_cleanup
            );
        }
    }
}
