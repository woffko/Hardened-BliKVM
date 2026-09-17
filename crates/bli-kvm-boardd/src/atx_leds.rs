//! ATX power and HDD LED inputs.
//!
//! The host's power and disk LEDs are wired into two board inputs. They are
//! read-only: the daemon samples them for the status view and never drives
//! them.

use bli_kvm_platform_bli::{GpioDriver, GpioRole, LineDirection, LineRequest};
use thiserror::Error;

use crate::config::AtxLedsConfig;

#[derive(Debug, Error)]
pub enum AtxLedError {
    #[error("ATX LED line failed")]
    Line,
}

/// Owns both ATX LED inputs and their last sampled levels.
pub struct AtxLedsRuntime<G: GpioDriver> {
    driver: G,
    power: LineRequest,
    hdd: LineRequest,
    power_on: bool,
    hdd_on: bool,
}

/// Reviewed read-only request for the host power LED input.
pub fn atx_power_led_line(config: &AtxLedsConfig) -> LineRequest {
    LineRequest {
        chip: "gpiochip0",
        offset: config.power.offset,
        consumer: "hardened-blikvm-boardd",
        role: GpioRole::AtxPowerLed,
        direction: LineDirection::Input,
        active_low: config.power.active_low,
    }
}

/// Reviewed read-only request for the host HDD LED input.
pub fn atx_hdd_led_line(config: &AtxLedsConfig) -> LineRequest {
    LineRequest {
        chip: "gpiochip0",
        offset: config.hdd.offset,
        consumer: "hardened-blikvm-boardd",
        role: GpioRole::AtxHddLed,
        direction: LineDirection::Input,
        active_low: config.hdd.active_low,
    }
}

impl<G: GpioDriver> AtxLedsRuntime<G> {
    pub fn new(
        mut driver: G,
        power: LineRequest,
        hdd: LineRequest,
        _config: &AtxLedsConfig,
    ) -> Result<Self, AtxLedError> {
        driver.request_line(&power).map_err(|_| AtxLedError::Line)?;
        driver.request_line(&hdd).map_err(|_| AtxLedError::Line)?;
        let power_on = driver.read_line(&power).unwrap_or(false);
        let hdd_on = driver.read_line(&hdd).unwrap_or(false);
        Ok(Self {
            driver,
            power,
            hdd,
            power_on,
            hdd_on,
        })
    }

    /// Read both inputs once; the status view reports the last good sample.
    pub fn sample(&mut self) -> Result<(), AtxLedError> {
        let power_on = self
            .driver
            .read_line(&self.power)
            .map_err(|_| AtxLedError::Line)?;
        let hdd_on = self
            .driver
            .read_line(&self.hdd)
            .map_err(|_| AtxLedError::Line)?;
        self.power_on = power_on;
        self.hdd_on = hdd_on;
        Ok(())
    }

    pub const fn power_on(&self) -> bool {
        self.power_on
    }

    pub const fn hdd_on(&self) -> bool {
        self.hdd_on
    }
}

impl<G: GpioDriver + Send> crate::server::AtxLedControl for AtxLedsRuntime<G> {
    fn sample(&mut self) -> Result<(), &'static str> {
        AtxLedsRuntime::sample(self).map_err(|_| "line")
    }

    fn power_present(&self) -> bool {
        true
    }

    fn power_on(&self) -> bool {
        self.power_on
    }

    fn hdd_present(&self) -> bool {
        true
    }

    fn hdd_on(&self) -> bool {
        self.hdd_on
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::config::LineSpec;

    #[derive(Default)]
    struct MockDriver {
        levels: Arc<Mutex<Vec<Result<bool, ()>>>>,
        requested: Arc<Mutex<Vec<u16>>>,
    }

    impl GpioDriver for MockDriver {
        type Error = ();

        fn request_line(&mut self, request: &LineRequest) -> Result<(), Self::Error> {
            self.requested.lock().map_err(|_| ())?.push(request.offset);
            Ok(())
        }

        fn write_line(&mut self, _request: &LineRequest, _active: bool) -> Result<(), Self::Error> {
            Err(())
        }

        fn read_line(&mut self, _request: &LineRequest) -> Result<bool, Self::Error> {
            let mut levels = self.levels.lock().map_err(|_| ())?;
            if levels.is_empty() {
                return Ok(false);
            }
            levels.remove(0)
        }

        fn release_line(&mut self, _request: &LineRequest) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn config() -> AtxLedsConfig {
        AtxLedsConfig {
            power: LineSpec {
                offset: 234,
                active_low: false,
            },
            hdd: LineSpec {
                offset: 233,
                active_low: false,
            },
        }
    }

    #[test]
    fn both_inputs_are_requested_and_sampled() {
        let levels = Arc::new(Mutex::new(vec![Ok(true), Ok(false), Ok(false), Ok(true)]));
        let requested = Arc::new(Mutex::new(Vec::new()));
        let driver = MockDriver {
            levels: Arc::clone(&levels),
            requested: Arc::clone(&requested),
        };
        let config = config();
        let mut runtime = AtxLedsRuntime::new(
            driver,
            atx_power_led_line(&config),
            atx_hdd_led_line(&config),
            &config,
        )
        .unwrap();
        assert_eq!(*requested.lock().unwrap(), vec![234, 233]);
        assert!(runtime.power_on());
        assert!(!runtime.hdd_on());
        runtime.sample().unwrap();
        assert!(!runtime.power_on());
        assert!(runtime.hdd_on());
        let power = atx_power_led_line(&config);
        assert_eq!(power.role, GpioRole::AtxPowerLed);
        assert_eq!(power.direction, LineDirection::Input);
        let hdd = atx_hdd_led_line(&config);
        assert_eq!(hdd.role, GpioRole::AtxHddLed);
    }

    #[test]
    fn a_failed_read_keeps_the_last_sample() {
        let levels = Arc::new(Mutex::new(vec![Ok(true), Ok(false), Err(())]));
        let driver = MockDriver {
            levels: Arc::clone(&levels),
            requested: Arc::new(Mutex::new(Vec::new())),
        };
        let config = config();
        let mut runtime = AtxLedsRuntime::new(
            driver,
            atx_power_led_line(&config),
            atx_hdd_led_line(&config),
            &config,
        )
        .unwrap();
        assert!(runtime.power_on());
        assert!(runtime.sample().is_err());
        assert!(runtime.power_on());
    }
}
