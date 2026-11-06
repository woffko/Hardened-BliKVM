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
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use bli_kvm_fan_supervisor::FanSupervisor;
use std::sync::Arc;

use bli_kvm_platform_bli::{GpioRole, LineDirection, LineRequest};
use config::BoarddConfig;
use fan::SoftPwmFan;
use server::{BoarddState, FanModeName};

/// Consumer label the daemon requests its lines with.
pub const BOARD_DAEMON_CONSUMER: &str = "hardened-blikvm-boardd";

/// Milliseconds on a monotonic clock since the daemon started. The fan,
/// button, LED and display policies refuse a time earlier than their last
/// one, so they never take the wall clock, which a network time step or a
/// manual correction moves back (code audit of 2026-10-20, AUD-06).
pub(crate) fn board_millis() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
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
    let mut supervisor = FanSupervisor::new(config.fan.policy_config(), board_millis(), fan)?;
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
    state.configure_display_sleep(bli_kvm_core::DISPLAY_SLEEP_PATH);
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
        board_millis(),
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
    match led::LedRuntime::new(driver, line, &config.led, board_millis()) {
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
    state.sample(board_millis())?;
    let view = state.status();
    println!("{}", serde_json::to_string(&view)?);
    Ok(())
}

/// How often the sampling thread turns: the activity LED's pulse (100 ms in
/// the reviewed configuration) and the display button's hold (1 s) need far
/// finer steps than the temperature's sample (code audit of 2026-11-06,
/// AUD-06: the LED lit for a second or not at all, and the button had to be
/// held two to three seconds).
const SAMPLER_TICK_MS: u64 = 20;

/// When the next temperature sample is due.
pub(crate) struct SampleCadence {
    interval_ms: u64,
    next_ms: Option<u64>,
}

impl SampleCadence {
    pub(crate) fn new(interval_ms: u64) -> Self {
        Self {
            interval_ms,
            next_ms: None,
        }
    }

    /// Whether a sample is due at `now_ms`; the next one is counted from it.
    pub(crate) fn due(&mut self, now_ms: u64) -> bool {
        if self.next_ms.is_some_and(|next| now_ms < next) {
            return false;
        }
        self.next_ms = Some(now_ms.saturating_add(self.interval_ms));
        true
    }
}

/// What a due sample read: the SoC's temperature and the external RTC.
pub(crate) struct Readings {
    pub(crate) thermal: Result<i32, &'static str>,
    pub(crate) rtc: Result<(i64, i64), &'static str>,
}

/// One turn of the sampling thread at `now_ms`: the LED and the display
/// button at every turn; the readings, the fan, the ATX LEDs and the display
/// when a sample was due.
pub(crate) fn board_turn<A>(
    state: &mut BoarddState<A>,
    now_ms: u64,
    readings: Option<Readings>,
) -> std::io::Result<()>
where
    A: bli_kvm_fan_supervisor::FanActuator,
    A::Error: std::fmt::Debug,
{
    let due = readings.is_some();
    if let Some(readings) = readings {
        state.note_thermal(readings.thermal);
        state.note_external_rtc(readings.rtc);
        let _ = state.sample(now_ms);
        // A failed background actuator is fatal in either fan mode. Stop
        // admission rather than serving stale supervisor health.
        state
            .ensure_fan_runtime()
            .map_err(|_| std::io::Error::other("boardd fan runtime failed"))?;
    }
    state.tick_led(now_ms);
    state.tick_button(now_ms);
    if due {
        state.tick_atx_leds();
        state.tick_display(now_ms);
    }
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
    let tick = std::time::Duration::from_millis(SAMPLER_TICK_MS.min(config.sample_interval_ms));
    let mut cadence = SampleCadence::new(config.sample_interval_ms);
    let sampler_state = Arc::clone(&state);
    let sample_stop = Arc::clone(&stopping);
    let serving = (|| -> Result<(), Box<dyn Error>> {
        let sampler = sampler::Sampler::spawn(Arc::clone(&stopping), tick, move || {
            let now = board_millis();
            // The readings are taken before the state's lock, as before.
            let readings = cadence.due(now).then(|| Readings {
                thermal: bli_kvm_platform_bli::read_h616_temperature_millidegrees()
                    .map_err(|error| error.class()),
                rtc: bli_kvm_platform_bli::read_h616_external_rtc()
                    .map(|readout| (readout.unix_seconds, readout.offset_from_system_seconds))
                    .map_err(|error| error.class()),
            });
            let mut state = sampler_state
                .lock()
                .map_err(|_| std::io::Error::other("boardd state poisoned"))?;
            if sample_stop.load(Ordering::SeqCst) {
                return Ok(());
            }
            board_turn(&mut state, now, readings)
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

#[cfg(test)]
mod turn_tests {
    //! The sampling thread's turns with the reviewed configuration
    //! (`packaging/boardd.json.example`), on lines driven by a shared clock
    //! (code audit of 2026-11-06, AUD-06).
    use super::*;
    use bli_kvm_fan_policy::FanCommand;
    use bli_kvm_fan_supervisor::FanActuator;
    use std::sync::atomic::AtomicU64;

    #[derive(Default)]
    struct NoFan;

    impl FanActuator for NoFan {
        type Error = std::io::Error;

        fn apply(&mut self, _command: FanCommand) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    /// A line on a shared clock: what is written is recorded with its time,
    /// and what is read is high between `high_from` and `high_until`.
    struct ClockedLine {
        clock: Arc<AtomicU64>,
        written: Arc<Mutex<Vec<(u64, bool)>>>,
        high_from: u64,
        high_until: u64,
    }

    impl bli_kvm_platform_bli::GpioDriver for ClockedLine {
        type Error = std::io::Error;

        fn request_line(&mut self, _request: &LineRequest) -> Result<(), Self::Error> {
            Ok(())
        }

        fn write_line(&mut self, _request: &LineRequest, active: bool) -> Result<(), Self::Error> {
            let now = self.clock.load(Ordering::SeqCst);
            self.written.lock().unwrap().push((now, active));
            Ok(())
        }

        fn read_line(&mut self, _request: &LineRequest) -> Result<bool, Self::Error> {
            let now = self.clock.load(Ordering::SeqCst);
            Ok((self.high_from..self.high_until).contains(&now))
        }

        fn release_line(&mut self, _request: &LineRequest) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn example() -> BoarddConfig {
        BoarddConfig::from_path(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packaging/boardd.json.example"
        )))
        .unwrap()
    }

    /// A board of the example configuration with its LED and button on the
    /// clock; the button is pressed between `pressed`.
    fn board(
        clock: &Arc<AtomicU64>,
        led_written: &Arc<Mutex<Vec<(u64, bool)>>>,
        pressed: (u64, u64),
    ) -> (BoarddState<NoFan>, BoarddConfig) {
        let config = example();
        let mut supervisor = FanSupervisor::new(config.fan.policy_config(), 0, NoFan).unwrap();
        let _ = supervisor.initialize();
        let led = led::LedRuntime::new(
            ClockedLine {
                clock: Arc::clone(clock),
                written: Arc::clone(led_written),
                high_from: 0,
                high_until: 0,
            },
            led::activity_led_line(&config.led),
            &config.led,
            0,
        )
        .unwrap();
        let button = button::ButtonRuntime::new(
            ClockedLine {
                clock: Arc::clone(clock),
                written: Arc::default(),
                high_from: pressed.0,
                high_until: pressed.1,
            },
            button::button_line(&config.button),
            &config.button,
            0,
            button::ButtonLevel::High,
        )
        .unwrap();
        let state = BoarddState::new(
            supervisor,
            FanModeName::Auto,
            0,
            20,
            100,
            true,
            40,
            269,
            None,
            Some(Box::new(led)),
            Some(Box::new(button)),
            None,
        );
        (state, config)
    }

    /// Turns from 0 to `until` at the thread's tick.
    fn turn(state: &mut BoarddState<NoFan>, config: &BoarddConfig, clock: &AtomicU64, until: u64) {
        let tick = SAMPLER_TICK_MS.min(config.sample_interval_ms);
        let mut cadence = SampleCadence::new(config.sample_interval_ms);
        let mut now = 0;
        while now <= until {
            clock.store(now, Ordering::SeqCst);
            let readings = cadence.due(now).then_some(Readings {
                thermal: Ok(40_000),
                rtc: Err("absent"),
            });
            board_turn(state, now, readings).unwrap();
            now += tick;
        }
    }

    #[test]
    fn the_activity_led_lights_its_pulse_once_a_period() {
        let clock = Arc::new(AtomicU64::new(0));
        let written = Arc::new(Mutex::new(Vec::new()));
        let (mut state, config) = board(&clock, &written, (0, 0));
        turn(&mut state, &config, &clock, 6_100);
        let changes = written.lock().unwrap().clone();
        let lit: Vec<(u64, u64)> = changes
            .windows(2)
            .filter(|pair| pair[0].1 && !pair[1].1)
            .map(|pair| (pair[0].0, pair[1].0 - pair[0].0))
            .collect();
        assert!(lit.len() >= 3, "lit windows {lit:?} from {changes:?}");
        for (start, length) in &lit {
            assert!(
                (80..=120).contains(length),
                "lit {length} ms at {start} ms (pulse {} ms): {lit:?}",
                config.led.pulse_ms
            );
        }
        for pair in lit.windows(2) {
            assert_eq!(pair[1].0 - pair[0].0, config.led.period_ms, "{lit:?}");
        }
    }

    #[test]
    fn a_press_held_for_its_hold_wakes_the_display_once() {
        let clock = Arc::new(AtomicU64::new(0));
        let written = Arc::new(Mutex::new(Vec::new()));
        // Held 1.1 s: the configured hold is 1 s.
        let (mut state, config) = board(&clock, &written, (500, 1_600));
        turn(&mut state, &config, &clock, 3_000);
        assert_eq!(state.status().button_wakeups, 1);
    }
}
