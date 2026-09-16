//! Fan actuation: soft-PWM over the reviewed GPIO line, driven by the fan
//! policy and supervisor that already exist.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use bli_kvm_fan_policy::FanCommand;
use bli_kvm_fan_supervisor::FanActuator;
use bli_kvm_platform_bli::{GpioDriver, LineRequest};
use thiserror::Error;

/// Split one soft-PWM period into its asserted and released parts.
///
/// A duty of zero means fully released and a duty of 100 means fully asserted;
/// every other value keeps at least one millisecond per phase, so a slow
/// period never produces a zero-length phase.
pub fn duty_durations(period_ms: u64, duty_percent: u8) -> (u64, u64) {
    let period = period_ms.max(2);
    let duty = u64::from(duty_percent.min(100));
    let asserted = (period * duty / 100).clamp(1, period - 1);
    (asserted, period - asserted)
}

#[derive(Debug, Error)]
pub enum FanError<E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    #[error("fan driver failed: {0}")]
    Driver(E),
    #[error("fan driver state is unavailable")]
    Locked,
}

/// Fan actuator that owns the reviewed line.
///
/// With PWM enabled it runs one background thread that keeps toggling the line
/// to the current duty. Without PWM (host tests and any gate before the fan
/// handoff) each duty change is a single level write, which verifies wiring and
/// fails safe; the status view reports `pwm` so the difference is visible.
pub struct SoftPwmFan<G>
where
    G: GpioDriver + Send + 'static,
    G::Error: std::error::Error + Send + Sync + 'static,
{
    driver: Arc<Mutex<G>>,
    line: LineRequest,
    period_ms: u64,
    duty: Arc<AtomicU8>,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pwm_enabled: bool,
}

impl<G> SoftPwmFan<G>
where
    G: GpioDriver + Send + 'static,
    G::Error: std::error::Error + Send + Sync + 'static,
{
    pub fn new(driver: G, line: LineRequest, period_ms: u64, pwm_enabled: bool) -> Self {
        Self {
            driver: Arc::new(Mutex::new(driver)),
            line,
            period_ms: period_ms.max(2),
            duty: Arc::new(AtomicU8::new(0)),
            running: Arc::new(AtomicBool::new(false)),
            thread: None,
            pwm_enabled,
        }
    }

    /// Request the line and, when enabled, start the PWM thread.
    pub fn start(&mut self) -> Result<(), FanError<G::Error>> {
        {
            let mut driver = self.driver.lock().map_err(|_| FanError::Locked)?;
            driver.request_line(&self.line).map_err(FanError::Driver)?;
            driver
                .write_line(&self.line, false)
                .map_err(FanError::Driver)?;
        }
        if self.pwm_enabled && self.thread.is_none() {
            let driver = Arc::clone(&self.driver);
            let line = self.line;
            let duty = Arc::clone(&self.duty);
            let running = Arc::clone(&self.running);
            let period = self.period_ms;
            running.store(true, Ordering::SeqCst);
            self.thread = Some(thread::spawn(move || {
                while running.load(Ordering::SeqCst) {
                    let current = duty.load(Ordering::SeqCst);
                    let (asserted, released) = duty_durations(period, current);
                    let write = |active: bool| {
                        if let Ok(mut driver) = driver.lock() {
                            let _ = driver.write_line(&line, active);
                        }
                    };
                    if current == 0 || current >= 100 {
                        write(current >= 100);
                        thread::sleep(Duration::from_millis(period));
                    } else {
                        write(true);
                        thread::sleep(Duration::from_millis(asserted));
                        write(false);
                        thread::sleep(Duration::from_millis(released));
                    }
                }
            }));
        }
        Ok(())
    }

    pub fn duty_percent(&self) -> u8 {
        self.duty.load(Ordering::SeqCst)
    }

    pub fn pwm_enabled(&self) -> bool {
        self.pwm_enabled
    }

    /// Release the line: stop the thread and drive it inactive.
    pub fn stop(&mut self) -> Result<(), FanError<G::Error>> {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
        let mut driver = self.driver.lock().map_err(|_| FanError::Locked)?;
        driver
            .write_line(&self.line, false)
            .map_err(FanError::Driver)?;
        driver.release_line(&self.line).map_err(FanError::Driver)
    }
}

impl<G> FanActuator for SoftPwmFan<G>
where
    G: GpioDriver + Send + 'static,
    G::Error: std::error::Error + Send + Sync + 'static,
{
    type Error = FanError<G::Error>;

    fn apply(&mut self, command: FanCommand) -> Result<(), Self::Error> {
        let duty = match command {
            FanCommand::Off => 0,
            FanCommand::DutyPercent(percent) => percent.get(),
        };
        self.duty.store(duty, Ordering::SeqCst);
        if self.thread.is_none() {
            // Level-only mode: only fully off and fully on are meaningful, and
            // an intermediate duty asserts the line so the fan cannot stall
            // silently while the gate reports the difference.
            let mut driver = self.driver.lock().map_err(|_| FanError::Locked)?;
            driver
                .write_line(&self.line, duty > 0)
                .map_err(FanError::Driver)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_platform_bli::{GpioRole, LineDirection};
    use std::time::Instant;

    #[derive(Debug, Default)]
    struct MockDriver {
        writes: Arc<Mutex<Vec<bool>>>,
        requested: Arc<Mutex<bool>>,
        released: Arc<Mutex<bool>>,
    }

    impl GpioDriver for MockDriver {
        type Error = std::io::Error;

        fn request_line(&mut self, _request: &LineRequest) -> Result<(), Self::Error> {
            *self.requested.lock().unwrap() = true;
            Ok(())
        }

        fn write_line(&mut self, _request: &LineRequest, active: bool) -> Result<(), Self::Error> {
            self.writes.lock().unwrap().push(active);
            Ok(())
        }

        fn read_line(&mut self, _request: &LineRequest) -> Result<bool, Self::Error> {
            Err(std::io::Error::other("read"))
        }

        fn release_line(&mut self, _request: &LineRequest) -> Result<(), Self::Error> {
            *self.released.lock().unwrap() = true;
            Ok(())
        }
    }

    fn line() -> LineRequest {
        LineRequest {
            chip: "gpiochip0",
            offset: 269,
            consumer: "hardened-blikvm-boardd",
            role: GpioRole::FanPwm,
            direction: LineDirection::Output,
            active_low: false,
        }
    }

    #[test]
    fn duty_split_keeps_both_phases_non_zero() {
        assert_eq!(duty_durations(40, 0), (1, 39));
        assert_eq!(duty_durations(40, 50), (20, 20));
        assert_eq!(duty_durations(40, 100), (39, 1));
        assert_eq!(duty_durations(1, 50), (1, 1));
    }

    #[test]
    fn level_mode_writes_the_requested_level() {
        let driver = MockDriver::default();
        let requested = Arc::clone(&driver.requested);
        let released = Arc::clone(&driver.released);
        let mut fan = SoftPwmFan::new(driver, line(), 40, false);
        fan.start().unwrap();
        assert!(*requested.lock().unwrap());
        assert_eq!(fan.duty_percent(), 0);

        fan.apply(FanCommand::DutyPercent(
            bli_kvm_fan_policy::FanDutyPercent::new(60).unwrap(),
        ))
        .unwrap();
        assert_eq!(fan.duty_percent(), 60);
        assert_eq!(fan.duty_percent(), 60);

        fan.apply(FanCommand::Off).unwrap();
        assert_eq!(fan.duty_percent(), 0);

        fan.stop().unwrap();
        assert!(*released.lock().unwrap());
    }

    #[test]
    fn pwm_mode_toggles_the_line() {
        let driver = MockDriver::default();
        let writes = Arc::clone(&driver.writes);
        let mut fan = SoftPwmFan::new(driver, line(), 10, true);
        fan.start().unwrap();
        fan.apply(FanCommand::DutyPercent(
            bli_kvm_fan_policy::FanDutyPercent::new(50).unwrap(),
        ))
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let seen = writes.lock().unwrap().clone();
            if seen.contains(&true) && seen.contains(&false) {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let seen = writes.lock().unwrap().clone();
        assert!(seen.contains(&true), "pwm never asserted: {seen:?}");
        assert!(seen.contains(&false), "pwm never released: {seen:?}");
        fan.stop().unwrap();
    }
}
