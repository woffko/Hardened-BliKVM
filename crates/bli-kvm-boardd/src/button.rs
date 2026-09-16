//! Display button runtime: samples the reviewed input line, debounces it with
//! `bli-kvm-display-button-policy` and reports the wake event.

use bli_kvm_display_button_policy::{
    ButtonOutcome, ButtonPolicyConfig, ButtonPolicyEngine, DisplayButtonEvent,
};

pub use bli_kvm_display_button_policy::ButtonLevel;
use bli_kvm_platform_bli::{GpioDriver, LineDirection, LineRequest};
use thiserror::Error;

use crate::config::ButtonConfig;

/// Consumer label the daemon requests the button line with.
const BUTTON_CONSUMER: &str = "hardened-blikvm-boardd";

#[derive(Debug, Error)]
pub enum ButtonError {
    #[error("display button policy is unavailable")]
    Policy,
    #[error("display button line failed")]
    Line,
}

/// Line request for the display button: an input line on the board chip.
pub fn button_line(config: &ButtonConfig) -> LineRequest {
    LineRequest {
        chip: "gpiochip0",
        offset: config.line.offset,
        consumer: BUTTON_CONSUMER,
        role: bli_kvm_platform_bli::GpioRole::DisplayButton,
        direction: LineDirection::Input,
        active_low: config.line.active_low,
    }
}

pub struct ButtonRuntime<G: GpioDriver> {
    driver: G,
    line: LineRequest,
    engine: ButtonPolicyEngine,
    active_high: bool,
    wakeups: u64,
}

impl<G: GpioDriver> ButtonRuntime<G> {
    pub fn new(
        mut driver: G,
        line: LineRequest,
        config: &ButtonConfig,
        started_at_ms: u64,
        initial_level: ButtonLevel,
    ) -> Result<Self, ButtonError> {
        driver.request_line(&line).map_err(|_| ButtonError::Line)?;
        let policy = ButtonPolicyConfig {
            active_level: if config.active_high {
                ButtonLevel::High
            } else {
                ButtonLevel::Low
            },
            debounce_ms: config.debounce_ms,
            activation_hold_ms: config.activation_hold_ms,
        };
        let engine = ButtonPolicyEngine::new(policy, started_at_ms, initial_level)
            .map_err(|_| ButtonError::Policy)?;
        Ok(Self {
            driver,
            line,
            engine,
            active_high: config.active_high,
            wakeups: 0,
        })
    }

    pub fn wakeups(&self) -> u64 {
        self.wakeups
    }

    /// Sample the line once and evaluate the policy.
    pub fn tick(&mut self, now_ms: u64) -> Result<Option<DisplayButtonEvent>, ButtonError> {
        let raw = self
            .driver
            .read_line(&self.line)
            .map_err(|_| ButtonError::Line)?;
        let level = if raw == self.active_high {
            ButtonLevel::High
        } else {
            ButtonLevel::Low
        };
        let outcome: ButtonOutcome = self
            .engine
            .evaluate(now_ms, level)
            .map_err(|_| ButtonError::Policy)?;
        if outcome.event == Some(DisplayButtonEvent::WakeDisplayCycle) {
            self.wakeups = self.wakeups.saturating_add(1);
        }
        Ok(outcome.event)
    }
}

impl<G: GpioDriver + Send> crate::server::ButtonControl for ButtonRuntime<G> {
    fn poll(&mut self, now_ms: u64) -> Result<bool, &'static str> {
        self.tick(now_ms)
            .map(|event| event == Some(DisplayButtonEvent::WakeDisplayCycle))
            .map_err(|_| "backend_unavailable")
    }

    fn wakeups(&self) -> u64 {
        ButtonRuntime::wakeups(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LineSpec;
    use std::sync::{Arc, Mutex};

    type Levels = Arc<Mutex<Vec<bool>>>;

    #[derive(Default)]
    struct MockDriver {
        levels: Levels,
    }

    impl GpioDriver for MockDriver {
        type Error = std::io::Error;

        fn request_line(&mut self, _request: &LineRequest) -> Result<(), Self::Error> {
            Ok(())
        }

        fn write_line(&mut self, _request: &LineRequest, _active: bool) -> Result<(), Self::Error> {
            Err(std::io::Error::other("output"))
        }

        fn read_line(&mut self, _request: &LineRequest) -> Result<bool, Self::Error> {
            let mut levels = self
                .levels
                .lock()
                .map_err(|_| std::io::Error::other("lock"))?;
            if levels.is_empty() {
                return Ok(false);
            }
            Ok(levels.remove(0))
        }

        fn release_line(&mut self, _request: &LineRequest) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn config() -> ButtonConfig {
        ButtonConfig {
            line: LineSpec {
                offset: 257,
                active_low: false,
            },
            active_high: true,
            debounce_ms: 50,
            activation_hold_ms: 1_000,
        }
    }

    #[test]
    fn button_line_is_an_input_line() {
        let line = button_line(&config());
        assert_eq!(line.offset, 257);
        assert_eq!(line.direction, LineDirection::Input);
    }

    #[test]
    fn a_held_press_requests_one_wake_cycle() {
        let driver = MockDriver {
            levels: Arc::new(Mutex::new(vec![false])),
        };
        let levels = Arc::clone(&driver.levels);
        let mut button = ButtonRuntime::new(
            driver,
            button_line(&config()),
            &config(),
            0,
            ButtonLevel::Low,
        )
        .unwrap();
        // Settle the startup sample, then hold the button high across the
        // debounce and activation windows.
        assert_eq!(button.tick(100).unwrap(), None);
        levels.lock().unwrap().extend([true, true, true, true]);
        let mut events = Vec::new();
        for now in [200_u64, 400, 1_200, 1_400] {
            events.push(button.tick(now).unwrap());
        }
        assert_eq!(
            events.iter().filter(|event| event.is_some()).count(),
            1,
            "one wake cycle expected: {events:?}"
        );
        assert_eq!(button.wakeups(), 1);
    }

    #[test]
    fn release_rearms_the_button() {
        let driver = MockDriver {
            levels: Arc::new(Mutex::new(vec![false])),
        };
        let levels = Arc::clone(&driver.levels);
        let mut button = ButtonRuntime::new(
            driver,
            button_line(&config()),
            &config(),
            0,
            ButtonLevel::Low,
        )
        .unwrap();
        button.tick(100).unwrap();
        levels
            .lock()
            .unwrap()
            .extend([true, true, true, true, false, false]);
        for now in [200_u64, 400, 1_200, 1_400, 1_600, 1_800] {
            let _ = button.tick(now).unwrap();
        }
        levels.lock().unwrap().extend([true, true, true, true]);
        for now in [2_000_u64, 2_200, 3_000, 3_200] {
            let _ = button.tick(now).unwrap();
        }
        assert_eq!(button.wakeups(), 2);
    }
}
