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
    #[error("fan shutdown requires the supervisor's full-duty failsafe")]
    FailsafeRequired,
    #[error("fan PWM worker panicked during shutdown")]
    WorkerPanicked,
    #[error("fan PWM worker failed; actuation is uncertain")]
    WorkerFailed,
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
    worker_failed: Arc<AtomicBool>,
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
            worker_failed: Arc::new(AtomicBool::new(false)),
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
            let worker_failed = Arc::clone(&self.worker_failed);
            let period = self.period_ms;
            running.store(true, Ordering::SeqCst);
            self.thread = Some(thread::spawn(move || {
                while running.load(Ordering::SeqCst) {
                    let current = duty.load(Ordering::SeqCst);
                    let (asserted, released) = duty_durations(period, current);
                    let write = |active: bool| {
                        match driver.lock() {
                            Ok(mut driver) => {
                                if driver.write_line(&line, active).is_ok() {
                                    return true;
                                }
                                worker_failed.store(true, Ordering::SeqCst);
                                // One bounded failsafe attempt. Failure remains
                                // latched even if this final assertion succeeds.
                                let _ = driver.write_line(&line, true);
                            }
                            Err(_) => worker_failed.store(true, Ordering::SeqCst),
                        }
                        false
                    };
                    if current == 0 || current >= 100 {
                        if !write(current >= 100) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(period));
                    } else {
                        if !write(true) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(asserted));
                        if !write(false) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(released));
                    }
                }
                running.store(false, Ordering::SeqCst);
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

    /// After prepare_shutdown latches full duty, stop PWM without undoing that
    /// failsafe. Retain the requested GPIO line until process/resource teardown.
    /// The electrical level after releasing the descriptor needs a device gate.
    pub fn quiesce_at_failsafe(&mut self) -> Result<(), FanError<G::Error>> {
        if self.duty_percent() != 100 {
            return Err(FanError::FailsafeRequired);
        }
        self.running.store(false, Ordering::SeqCst);
        let panicked = self
            .thread
            .take()
            .is_some_and(|handle| handle.join().is_err());
        if panicked {
            self.worker_failed.store(true, Ordering::SeqCst);
        }
        // A PWM phase may have written inactive after the stop flag. Reassert
        // only after join, so no later PWM write can undo the final level.
        self.driver
            .lock()
            .map_err(|_| FanError::Locked)?
            .write_line(&self.line, true)
            .map_err(FanError::Driver)?;
        if panicked {
            Err(FanError::WorkerPanicked)
        } else if self.worker_failed.load(Ordering::SeqCst) {
            Err(FanError::WorkerFailed)
        } else {
            Ok(())
        }
    }

    /// Release the line: stop the thread and drive it inactive.
    pub fn stop(&mut self) -> Result<(), FanError<G::Error>> {
        self.running.store(false, Ordering::SeqCst);
        let panicked = self
            .thread
            .take()
            .is_some_and(|handle| handle.join().is_err());
        if panicked {
            self.worker_failed.store(true, Ordering::SeqCst);
        }
        let mut driver = self.driver.lock().map_err(|_| FanError::Locked)?;
        driver
            .write_line(&self.line, false)
            .map_err(FanError::Driver)?;
        driver.release_line(&self.line).map_err(FanError::Driver)?;
        if panicked {
            Err(FanError::WorkerPanicked)
        } else if self.worker_failed.load(Ordering::SeqCst) {
            Err(FanError::WorkerFailed)
        } else {
            Ok(())
        }
    }
}

impl<G> Drop for SoftPwmFan<G>
where
    G: GpioDriver + Send + 'static,
    G::Error: std::error::Error + Send + Sync + 'static,
{
    fn drop(&mut self) {
        if self.thread.is_some() {
            // Startup failure/unwind fallback. The worker owns a driver Arc;
            // dropping its JoinHandle would detach it and retain the GPIO line.
            // Use the existing full-duty shutdown failsafe, never an off pulse.
            // Normal shutdown explicitly quiesces and reports errors beforehand.
            self.duty.store(100, Ordering::SeqCst);
            if let Err(error) = self.quiesce_at_failsafe() {
                eprintln!("boardd: emergency fan shutdown failed: {error}");
            }
        }
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
        self.check_runtime()?;
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

    fn check_runtime(&self) -> Result<(), Self::Error> {
        if self.worker_failed.load(Ordering::SeqCst)
            || self.driver.is_poisoned()
            || (self.pwm_enabled && self.thread.as_ref().is_none_or(JoinHandle::is_finished))
        {
            Err(FanError::WorkerFailed)
        } else {
            Ok(())
        }
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
        fail_write: Arc<AtomicBool>,
    }

    impl GpioDriver for MockDriver {
        type Error = std::io::Error;

        fn request_line(&mut self, _request: &LineRequest) -> Result<(), Self::Error> {
            *self.requested.lock().unwrap() = true;
            Ok(())
        }

        fn write_line(&mut self, _request: &LineRequest, active: bool) -> Result<(), Self::Error> {
            if self.fail_write.load(Ordering::SeqCst) {
                return Err(std::io::Error::other("fixture write failure"));
            }
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

    #[test]
    fn shutdown_joins_pwm_and_retains_the_asserted_line() {
        let driver = MockDriver::default();
        let writes = driver.writes.clone();
        let released = driver.released.clone();
        let mut fan = SoftPwmFan::new(driver, line(), 10, true);
        fan.start().unwrap();
        assert!(matches!(
            fan.quiesce_at_failsafe(),
            Err(FanError::FailsafeRequired)
        ));
        fan.apply(FanCommand::DutyPercent(
            bli_kvm_fan_policy::FanDutyPercent::new(100).unwrap(),
        ))
        .unwrap();
        fan.quiesce_at_failsafe().unwrap();
        assert!(fan.thread.is_none());
        assert!(!fan.running.load(Ordering::SeqCst));
        assert_eq!(writes.lock().unwrap().last(), Some(&true));
        assert!(!*released.lock().unwrap());
        let count = writes.lock().unwrap().len();
        thread::sleep(Duration::from_millis(25));
        assert_eq!(
            writes.lock().unwrap().len(),
            count,
            "late PWM write after join"
        );
        fan.quiesce_at_failsafe().unwrap();
        assert_eq!(writes.lock().unwrap().last(), Some(&true));
    }

    #[test]
    fn failed_worker_is_not_reported_as_clean_even_after_failsafe_write() {
        let driver = MockDriver::default();
        let writes = driver.writes.clone();
        let mut fan = SoftPwmFan::new(driver, line(), 10, false);
        fan.start().unwrap();
        fan.apply(FanCommand::DutyPercent(
            bli_kvm_fan_policy::FanDutyPercent::new(100).unwrap(),
        ))
        .unwrap();
        fan.thread = Some(thread::spawn(|| panic!("fixture worker failure")));
        assert!(matches!(
            fan.quiesce_at_failsafe(),
            Err(FanError::WorkerPanicked)
        ));
        assert_eq!(writes.lock().unwrap().last(), Some(&true));
    }

    #[test]
    fn final_failsafe_write_failure_is_reported() {
        let driver = MockDriver::default();
        let fail_write = driver.fail_write.clone();
        let mut fan = SoftPwmFan::new(driver, line(), 10, false);
        fan.start().unwrap();
        fan.apply(FanCommand::DutyPercent(
            bli_kvm_fan_policy::FanDutyPercent::new(100).unwrap(),
        ))
        .unwrap();
        fail_write.store(true, Ordering::SeqCst);
        assert!(matches!(
            fan.quiesce_at_failsafe(),
            Err(FanError::Driver(_))
        ));
    }

    #[test]
    fn dropping_active_pwm_joins_and_does_not_retain_the_driver() {
        let driver = MockDriver::default();
        let writes = driver.writes.clone();
        let mut fan = SoftPwmFan::new(driver, line(), 10, true);
        let weak_driver = Arc::downgrade(&fan.driver);
        fan.start().unwrap();
        fan.apply(FanCommand::DutyPercent(
            bli_kvm_fan_policy::FanDutyPercent::new(50).unwrap(),
        ))
        .unwrap();
        drop(fan);
        assert!(
            weak_driver.upgrade().is_none(),
            "detached worker retained driver"
        );
        assert_eq!(writes.lock().unwrap().last(), Some(&true));
    }

    #[test]
    fn dropping_quiesced_pwm_does_not_add_unreported_hardware_writes() {
        let driver = MockDriver::default();
        let writes = driver.writes.clone();
        let mut fan = SoftPwmFan::new(driver, line(), 10, true);
        fan.start().unwrap();
        fan.apply(FanCommand::DutyPercent(
            bli_kvm_fan_policy::FanDutyPercent::new(100).unwrap(),
        ))
        .unwrap();
        fan.quiesce_at_failsafe().unwrap();
        let count = writes.lock().unwrap().len();
        drop(fan);
        assert_eq!(writes.lock().unwrap().len(), count);
    }

    #[test]
    fn pwm_write_failure_is_latched_after_worker_exit_and_final_write() {
        let driver = MockDriver::default();
        let fail_write = driver.fail_write.clone();
        let writes = driver.writes.clone();
        let mut fan = SoftPwmFan::new(driver, line(), 10, true);
        fan.start().unwrap();
        assert!(fan.check_runtime().is_ok());
        fail_write.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !fan.thread.as_ref().unwrap().is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            fan.thread.as_ref().unwrap().is_finished(),
            "failed PWM did not stop"
        );
        fail_write.store(false, Ordering::SeqCst);
        let count = writes.lock().unwrap().len();
        assert!(matches!(fan.check_runtime(), Err(FanError::WorkerFailed)));
        assert_eq!(
            writes.lock().unwrap().len(),
            count,
            "health check must not actuate"
        );
        assert!(matches!(
            fan.apply(FanCommand::DutyPercent(
                bli_kvm_fan_policy::FanDutyPercent::new(100).unwrap(),
            )),
            Err(FanError::WorkerFailed)
        ));
        assert!(matches!(
            fan.quiesce_at_failsafe(),
            Err(FanError::WorkerFailed)
        ));
        assert_eq!(writes.lock().unwrap().last(), Some(&true));
        assert!(matches!(
            fan.quiesce_at_failsafe(),
            Err(FanError::WorkerFailed)
        ));
    }
}
