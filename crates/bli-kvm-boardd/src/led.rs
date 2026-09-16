//! Activity LED pulse runtime over the reviewed line.
//!
//! The pulse shape is a policy decision (`bli-kvm-activity-led-policy`); this
//! module only keeps the line in step with it and exposes the mode.

use bli_kvm_activity_led_policy::{
    ActivityLedConfig, ActivityLedDecision, ActivityLedMode, ActivityLedPolicyEngine,
    ActivityLedPolicyError, OutputLevel,
};
use bli_kvm_platform_bli::{GpioDriver, LineRequest};
use thiserror::Error;

use crate::config::LedConfig;

#[derive(Debug, Error)]
pub enum LedError {
    #[error("activity LED policy is unavailable")]
    Policy,
    #[error("activity LED line failed")]
    Line,
}

/// Owns the LED line and one policy engine.
pub struct LedRuntime<G: GpioDriver> {
    driver: G,
    line: LineRequest,
    engine: ActivityLedPolicyEngine,
    asserted_high: bool,
    mode: ActivityLedMode,
    last_level: Option<OutputLevel>,
    current_active: bool,
}

/// Reviewed line request for the activity LED.
pub fn activity_led_line(config: &LedConfig) -> LineRequest {
    LineRequest {
        chip: "gpiochip0",
        offset: config.line.offset,
        consumer: "hardened-blikvm-boardd",
        role: bli_kvm_platform_bli::GpioRole::ActivityLed,
        direction: bli_kvm_platform_bli::LineDirection::Output,
        active_low: config.line.active_low,
    }
}

impl<G: GpioDriver> LedRuntime<G> {
    pub fn new(
        mut driver: G,
        line: LineRequest,
        config: &LedConfig,
        started_at_ms: u64,
    ) -> Result<Self, LedError> {
        driver.request_line(&line).map_err(|_| LedError::Line)?;
        let policy = ActivityLedConfig {
            asserted_level: if config.asserted_high {
                OutputLevel::High
            } else {
                OutputLevel::Low
            },
            period_ms: config.period_ms,
            pulse_ms: config.pulse_ms,
        };
        let mode = if config.enabled {
            ActivityLedMode::Enabled
        } else {
            ActivityLedMode::Disabled
        };
        let mut runtime = Self {
            driver,
            line,
            engine: ActivityLedPolicyEngine::new(policy, started_at_ms)
                .map_err(|_| LedError::Policy)?,
            asserted_high: config.asserted_high,
            mode,
            last_level: None,
            current_active: false,
        };
        runtime.apply(mode, started_at_ms)?;
        Ok(runtime)
    }

    pub fn mode(&self) -> ActivityLedMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: ActivityLedMode, now_ms: u64) -> Result<(), LedError> {
        self.apply(mode, now_ms)
    }

    /// One pulse evaluation; writes the line only when the level changes.
    pub fn tick(&mut self, now_ms: u64) -> Result<(), LedError> {
        let mode = self.mode;
        self.apply(mode, now_ms)
    }

    fn apply(&mut self, mode: ActivityLedMode, now_ms: u64) -> Result<(), LedError> {
        let decision: ActivityLedDecision = self
            .engine
            .evaluate(now_ms, mode)
            .map_err(|_: ActivityLedPolicyError| LedError::Policy)?;
        self.mode = mode;
        if self.last_level != Some(decision.level) || decision.command_changed {
            let active = (decision.level == OutputLevel::High) == self.asserted_high;
            self.driver
                .write_line(&self.line, active)
                .map_err(|_| LedError::Line)?;
            self.last_level = Some(decision.level);
            self.current_active = active;
        }
        Ok(())
    }

    /// Whether the line is currently asserted.
    pub fn level(&self) -> bool {
        self.current_active
    }

    /// Whether the pulse is enabled.
    pub fn enabled(&self) -> bool {
        self.mode == ActivityLedMode::Enabled
    }
}

impl<G: GpioDriver + Send> crate::server::LedControl for LedRuntime<G> {
    fn set_enabled(&mut self, enabled: bool, now_ms: u64) -> Result<(), &'static str> {
        let mode = if enabled {
            ActivityLedMode::Enabled
        } else {
            ActivityLedMode::Disabled
        };
        self.set_mode(mode, now_ms)
            .map_err(|_| "backend_unavailable")
    }

    fn enabled(&self) -> bool {
        LedRuntime::enabled(self)
    }

    fn level(&self) -> bool {
        LedRuntime::level(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LineSpec;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct MockDriver {
        writes: Arc<Mutex<Vec<(u16, bool)>>>,
    }

    impl GpioDriver for MockDriver {
        type Error = std::io::Error;

        fn request_line(&mut self, _request: &LineRequest) -> Result<(), Self::Error> {
            Ok(())
        }

        fn write_line(&mut self, request: &LineRequest, active: bool) -> Result<(), Self::Error> {
            self.writes.lock().unwrap().push((request.offset, active));
            Ok(())
        }

        fn read_line(&mut self, _request: &LineRequest) -> Result<bool, Self::Error> {
            Err(std::io::Error::other("read"))
        }

        fn release_line(&mut self, _request: &LineRequest) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn line() -> LineRequest {
        LineRequest {
            chip: "gpiochip0",
            offset: 261,
            consumer: "hardened-blikvm-boardd",
            role: bli_kvm_platform_bli::GpioRole::ActivityLed,
            direction: bli_kvm_platform_bli::LineDirection::Output,
            active_low: false,
        }
    }

    fn config(enabled: bool) -> LedConfig {
        LedConfig {
            line: LineSpec {
                offset: 261,
                active_low: false,
            },
            asserted_high: true,
            period_ms: 200,
            pulse_ms: 50,
            enabled,
        }
    }

    #[test]
    fn disabled_led_stays_at_the_inactive_level() {
        let driver = MockDriver::default();
        let writes = Arc::clone(&driver.writes);
        let mut led = LedRuntime::new(driver, line(), &config(false), 0).unwrap();
        assert_eq!(led.mode(), ActivityLedMode::Disabled);
        let initial = writes.lock().unwrap().clone();
        assert_eq!(initial, vec![(261, false)]);
        led.tick(1_000).unwrap();
        assert_eq!(writes.lock().unwrap().len(), 1, "no extra writes when idle");
    }

    #[test]
    fn enabled_led_pulses_across_the_period() {
        let driver = MockDriver::default();
        let writes = Arc::clone(&driver.writes);
        let mut led = LedRuntime::new(driver, line(), &config(true), 0).unwrap();
        assert_eq!(led.mode(), ActivityLedMode::Enabled);
        // Pulse at the start of the period, idle later in the period.
        led.tick(10).unwrap();
        led.tick(100).unwrap();
        let levels: Vec<bool> = writes
            .lock()
            .unwrap()
            .iter()
            .map(|(_, level)| *level)
            .collect();
        assert!(levels.contains(&true), "pulse never asserted: {levels:?}");
        assert!(levels.contains(&false), "pulse never released: {levels:?}");
    }

    #[test]
    fn mode_changes_reach_the_engine() {
        let driver = MockDriver::default();
        let mut led = LedRuntime::new(driver, line(), &config(true), 0).unwrap();
        led.set_mode(ActivityLedMode::Disabled, 500).unwrap();
        assert_eq!(led.mode(), ActivityLedMode::Disabled);
        led.set_mode(ActivityLedMode::Enabled, 1_000).unwrap();
        assert_eq!(led.mode(), ActivityLedMode::Enabled);
    }
}
