//! GPIO-backed ATX pulse executor.
//!
//! Drives the reviewed ATX power/reset lines through the platform GPIO driver
//! seam: request the line, drive it active for the configured hold time, drive
//! it inactive, and always release it. The hold times are configuration, not
//! hardware facts, so the executor is exercised with a mock driver and a mock
//! sleeper and this crate performs no I/O on its own. The concrete owner runs
//! only after the reviewed physical line handoff.

use bli_kvm_atx_transition::AtxPulseRunner;
use bli_kvm_core::AtxControlAction;
use bli_kvm_platform_bli::{GpioDriver, GpioRole, LineDirection, LineRequest};
use thiserror::Error;

const MAX_HOLD_MS: u64 = 10_000;

/// Pulse hold times, in milliseconds. Supplied by the reviewed owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtxTiming {
    pub power_short_ms: u64,
    pub power_long_ms: u64,
    pub reset_ms: u64,
}

impl AtxTiming {
    fn validate(&self) -> Result<(), AtxGpioError> {
        for hold in [self.power_short_ms, self.power_long_ms, self.reset_ms] {
            if hold == 0 || hold > MAX_HOLD_MS {
                return Err(AtxGpioError::InvalidTiming);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AtxGpioError {
    #[error("ATX pulse timing is out of range")]
    InvalidTiming,
    #[error("ATX line is not a reviewed actuator output")]
    InvalidLine,
    #[error("ATX line request failed: {0}")]
    Request(String),
    #[error("ATX line driver failed: {0}")]
    Driver(String),
}

/// Time seam so the pulse can be tested without sleeping.
pub trait Sleeper {
    fn sleep_ms(&mut self, ms: u64);
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RealSleeper;

impl Sleeper for RealSleeper {
    fn sleep_ms(&mut self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

pub struct AtxGpioExecutor<G, S> {
    driver: G,
    sleeper: S,
    power: LineRequest,
    reset: LineRequest,
    timing: AtxTiming,
}

impl<G: GpioDriver, S: Sleeper> AtxGpioExecutor<G, S> {
    pub fn new(
        driver: G,
        sleeper: S,
        power: LineRequest,
        reset: LineRequest,
        timing: AtxTiming,
    ) -> Result<Self, AtxGpioError> {
        if power.role != GpioRole::AtxPower
            || reset.role != GpioRole::AtxReset
            || power.direction != LineDirection::Output
            || reset.direction != LineDirection::Output
        {
            return Err(AtxGpioError::InvalidLine);
        }
        timing.validate()?;
        Ok(Self {
            driver,
            sleeper,
            power,
            reset,
            timing,
        })
    }

    /// Apply one reviewed action as a single bounded pulse.
    pub fn pulse(&mut self, action: AtxControlAction) -> Result<(), AtxGpioError> {
        let (line, hold) = match action {
            AtxControlAction::PowerShort => (self.power, self.timing.power_short_ms),
            AtxControlAction::PowerLong => (self.power, self.timing.power_long_ms),
            AtxControlAction::Reset => (self.reset, self.timing.reset_ms),
        };

        self.driver
            .request_line(&line)
            .map_err(|error| AtxGpioError::Request(format!("{error:?}")))?;

        let active = self.driver.write_line(&line, true);
        if active.is_ok() {
            self.sleeper.sleep_ms(hold);
        }
        let inactive = self.driver.write_line(&line, false);
        let released = self.driver.release_line(&line);

        active.map_err(|error| AtxGpioError::Driver(format!("{error:?}")))?;
        inactive.map_err(|error| AtxGpioError::Driver(format!("{error:?}")))?;
        released.map_err(|error| AtxGpioError::Driver(format!("{error:?}")))?;
        Ok(())
    }
}

impl<G: GpioDriver + Send, S: Sleeper + Send> AtxPulseRunner for AtxGpioExecutor<G, S> {
    fn execute(&mut self, action: AtxControlAction) -> Result<(), String> {
        self.pulse(action).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_platform_bli::hardened_board_request;

    #[derive(Default)]
    struct MockDriver {
        fail_request: bool,
        fail_first_write: bool,
        write_failed: bool,
        events: Vec<String>,
    }

    impl GpioDriver for MockDriver {
        type Error = &'static str;

        fn request_line(&mut self, request: &LineRequest) -> Result<(), Self::Error> {
            if self.fail_request {
                return Err("request");
            }
            self.events.push(format!("request:{}", request.offset));
            Ok(())
        }

        fn write_line(&mut self, request: &LineRequest, active: bool) -> Result<(), Self::Error> {
            if self.fail_first_write && !self.write_failed {
                self.write_failed = true;
                return Err("write");
            }
            self.events
                .push(format!("write:{}:{active}", request.offset));
            Ok(())
        }

        fn release_line(&mut self, request: &LineRequest) -> Result<(), Self::Error> {
            self.events.push(format!("release:{}", request.offset));
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockSleeper {
        sleeps: Vec<u64>,
    }

    impl Sleeper for MockSleeper {
        fn sleep_ms(&mut self, ms: u64) {
            self.sleeps.push(ms);
        }
    }

    fn timing() -> AtxTiming {
        AtxTiming {
            power_short_ms: 500,
            power_long_ms: 4_000,
            reset_ms: 200,
        }
    }

    fn executor(
        driver: MockDriver,
        sleeper: MockSleeper,
    ) -> AtxGpioExecutor<MockDriver, MockSleeper> {
        AtxGpioExecutor::new(
            driver,
            sleeper,
            hardened_board_request(GpioRole::AtxPower).unwrap(),
            hardened_board_request(GpioRole::AtxReset).unwrap(),
            timing(),
        )
        .unwrap()
    }

    #[test]
    fn power_short_pulses_the_power_line_and_releases_it() {
        let mut executor = executor(MockDriver::default(), MockSleeper::default());
        assert_eq!(executor.pulse(AtxControlAction::PowerShort), Ok(()));
        let driver = executor.driver;
        let sleeper = executor.sleeper;
        assert_eq!(
            driver.events,
            vec![
                "request:228".to_string(),
                "write:228:true".to_string(),
                "write:228:false".to_string(),
                "release:228".to_string(),
            ]
        );
        assert_eq!(sleeper.sleeps, vec![500]);
    }

    #[test]
    fn power_long_and_reset_use_their_own_timings_and_lines() {
        let mut long = executor(MockDriver::default(), MockSleeper::default());
        assert_eq!(long.pulse(AtxControlAction::PowerLong), Ok(()));
        assert_eq!(long.sleeper.sleeps, vec![4_000]);

        let mut reset = executor(MockDriver::default(), MockSleeper::default());
        assert_eq!(reset.pulse(AtxControlAction::Reset), Ok(()));
        assert_eq!(reset.sleeper.sleeps, vec![200]);
        assert!(reset.driver.events[0].eq("request:272"));
    }

    #[test]
    fn a_failed_active_write_still_drives_inactive_and_releases() {
        let mut executor = executor(
            MockDriver {
                fail_first_write: true,
                ..MockDriver::default()
            },
            MockSleeper::default(),
        );
        assert!(matches!(
            executor.pulse(AtxControlAction::PowerShort),
            Err(AtxGpioError::Driver(_))
        ));
        assert!(executor.sleeper.sleeps.is_empty());
        assert_eq!(
            executor.driver.events,
            vec![
                "request:228".to_string(),
                "write:228:false".to_string(),
                "release:228".to_string(),
            ]
        );
    }

    #[test]
    fn a_failed_request_does_not_write() {
        let mut executor = executor(
            MockDriver {
                fail_request: true,
                ..MockDriver::default()
            },
            MockSleeper::default(),
        );
        assert!(matches!(
            executor.pulse(AtxControlAction::Reset),
            Err(AtxGpioError::Request(_))
        ));
        assert!(executor.driver.events.is_empty());
    }

    #[test]
    fn construction_rejects_wrong_lines_and_timings() {
        let power = hardened_board_request(GpioRole::AtxPower).unwrap();
        let reset = hardened_board_request(GpioRole::AtxReset).unwrap();
        assert!(matches!(
            AtxGpioExecutor::new(
                MockDriver::default(),
                MockSleeper::default(),
                reset,
                reset,
                timing()
            ),
            Err(AtxGpioError::InvalidLine)
        ));
        assert!(matches!(
            AtxGpioExecutor::new(
                MockDriver::default(),
                MockSleeper::default(),
                power,
                reset,
                AtxTiming {
                    power_short_ms: 0,
                    power_long_ms: 4_000,
                    reset_ms: 200,
                }
            ),
            Err(AtxGpioError::InvalidTiming)
        ));
        assert!(matches!(
            AtxGpioExecutor::new(
                MockDriver::default(),
                MockSleeper::default(),
                power,
                reset,
                AtxTiming {
                    power_short_ms: 500,
                    power_long_ms: 10_001,
                    reset_ms: 200,
                }
            ),
            Err(AtxGpioError::InvalidTiming)
        ));
    }

    #[test]
    fn executor_satisfies_the_pulse_runner_trait() {
        let mut runner: Box<dyn AtxPulseRunner> =
            Box::new(executor(MockDriver::default(), MockSleeper::default()));
        assert_eq!(runner.execute(AtxControlAction::Reset), Ok(()));
    }
}
