#![no_std]
#![forbid(unsafe_code)]

//! Pure explicit-polarity activity LED schedule policy.
//!
//! Callers supply monotonic time and an explicit enabled/disabled/fault mode.
//! The returned level is a policy command only; this crate has no GPIO, clock,
//! filesystem, thread, network, or device access.

pub const MIN_PERIOD_MS: u64 = 100;
pub const MAX_PERIOD_MS: u64 = 60_000;
pub const MIN_PULSE_MS: u64 = 10;
pub const MAX_PULSE_MS: u64 = 5_000;
pub const MIN_IDLE_MS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputLevel {
    Low,
    High,
}

impl OutputLevel {
    pub const fn opposite(self) -> Self {
        match self {
            Self::Low => Self::High,
            Self::High => Self::Low,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivityLedConfig {
    pub asserted_level: OutputLevel,
    pub period_ms: u64,
    pub pulse_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityLedConfigError {
    PeriodOutOfRange,
    PulseOutOfRange,
    InsufficientIdleTime,
}

impl ActivityLedConfig {
    pub const fn validate(self) -> Result<(), ActivityLedConfigError> {
        if self.period_ms < MIN_PERIOD_MS || self.period_ms > MAX_PERIOD_MS {
            return Err(ActivityLedConfigError::PeriodOutOfRange);
        }
        if self.pulse_ms < MIN_PULSE_MS || self.pulse_ms > MAX_PULSE_MS {
            return Err(ActivityLedConfigError::PulseOutOfRange);
        }
        if self.pulse_ms >= self.period_ms || self.period_ms - self.pulse_ms < MIN_IDLE_MS {
            return Err(ActivityLedConfigError::InsufficientIdleTime);
        }
        Ok(())
    }

    pub const fn inactive_level(self) -> OutputLevel {
        self.asserted_level.opposite()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActivityLedMode {
    Disabled,
    Enabled,
    Fault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityLedReason {
    DisabledInactive,
    FaultInactive,
    EnabledPulse,
    EnabledIdle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityLedPolicyError {
    ClockMovedBackwards,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivityLedState {
    pub mode: ActivityLedMode,
    pub level: OutputLevel,
    pub enabled_at_ms: Option<u64>,
    pub last_evaluation_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivityLedDecision {
    pub level: OutputLevel,
    pub reason: ActivityLedReason,
    pub command_changed: bool,
    pub mode_changed: bool,
    pub period_position_ms: Option<u64>,
    pub state: ActivityLedState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityLedPolicyEngine {
    config: ActivityLedConfig,
    mode: ActivityLedMode,
    level: OutputLevel,
    enabled_at_ms: Option<u64>,
    last_evaluation_ms: u64,
}

impl ActivityLedPolicyEngine {
    pub fn new(
        config: ActivityLedConfig,
        started_at_ms: u64,
    ) -> Result<Self, ActivityLedConfigError> {
        config.validate()?;
        Ok(Self {
            config,
            mode: ActivityLedMode::Disabled,
            level: config.inactive_level(),
            enabled_at_ms: None,
            last_evaluation_ms: started_at_ms,
        })
    }

    pub const fn state(&self) -> ActivityLedState {
        ActivityLedState {
            mode: self.mode,
            level: self.level,
            enabled_at_ms: self.enabled_at_ms,
            last_evaluation_ms: self.last_evaluation_ms,
        }
    }

    pub fn evaluate(
        &mut self,
        now_ms: u64,
        requested_mode: ActivityLedMode,
    ) -> Result<ActivityLedDecision, ActivityLedPolicyError> {
        if now_ms < self.last_evaluation_ms {
            return Err(ActivityLedPolicyError::ClockMovedBackwards);
        }

        let previous_mode = self.mode;
        let previous_level = self.level;
        if requested_mode != self.mode {
            self.mode = requested_mode;
            self.enabled_at_ms = if requested_mode == ActivityLedMode::Enabled {
                Some(now_ms)
            } else {
                None
            };
        }

        let period_position_ms;
        let reason;
        self.level = match self.mode {
            ActivityLedMode::Disabled => {
                period_position_ms = None;
                reason = ActivityLedReason::DisabledInactive;
                self.config.inactive_level()
            }
            ActivityLedMode::Fault => {
                period_position_ms = None;
                reason = ActivityLedReason::FaultInactive;
                self.config.inactive_level()
            }
            ActivityLedMode::Enabled => {
                let enabled_at_ms = self.enabled_at_ms.unwrap_or(now_ms);
                let position = now_ms.saturating_sub(enabled_at_ms) % self.config.period_ms;
                period_position_ms = Some(position);
                if position < self.config.pulse_ms {
                    reason = ActivityLedReason::EnabledPulse;
                    self.config.asserted_level
                } else {
                    reason = ActivityLedReason::EnabledIdle;
                    self.config.inactive_level()
                }
            }
        };
        self.last_evaluation_ms = now_ms;

        Ok(ActivityLedDecision {
            level: self.level,
            reason,
            command_changed: self.level != previous_level,
            mode_changed: self.mode != previous_mode,
            period_position_ms,
            state: self.state(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vendor_shaped_config() -> ActivityLedConfig {
        ActivityLedConfig {
            asserted_level: OutputLevel::Low,
            period_ms: 2_000,
            pulse_ms: 100,
        }
    }

    fn evaluate(
        engine: &mut ActivityLedPolicyEngine,
        now_ms: u64,
        mode: ActivityLedMode,
    ) -> ActivityLedDecision {
        engine.evaluate(now_ms, mode).unwrap()
    }

    #[test]
    fn configuration_requires_explicit_bounded_pulse_and_idle() {
        assert_eq!(
            ActivityLedConfig {
                asserted_level: OutputLevel::High,
                period_ms: 99,
                pulse_ms: 10,
            }
            .validate(),
            Err(ActivityLedConfigError::PeriodOutOfRange)
        );
        assert_eq!(
            ActivityLedConfig {
                asserted_level: OutputLevel::High,
                period_ms: 1_000,
                pulse_ms: 9,
            }
            .validate(),
            Err(ActivityLedConfigError::PulseOutOfRange)
        );
        assert_eq!(
            ActivityLedConfig {
                asserted_level: OutputLevel::High,
                period_ms: 100,
                pulse_ms: 95,
            }
            .validate(),
            Err(ActivityLedConfigError::InsufficientIdleTime)
        );
        assert!(
            ActivityLedConfig {
                asserted_level: OutputLevel::Low,
                period_ms: MAX_PERIOD_MS,
                pulse_ms: MAX_PULSE_MS,
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn source_shaped_schedule_has_exact_edges_without_drift() {
        let mut engine = ActivityLedPolicyEngine::new(vendor_shaped_config(), 0).unwrap();
        assert_eq!(engine.state().mode, ActivityLedMode::Disabled);
        assert_eq!(engine.state().level, OutputLevel::High);

        let enabled = evaluate(&mut engine, 10, ActivityLedMode::Enabled);
        assert_eq!(enabled.level, OutputLevel::Low);
        assert_eq!(enabled.reason, ActivityLedReason::EnabledPulse);
        assert_eq!(enabled.period_position_ms, Some(0));
        assert!(enabled.command_changed);
        assert!(enabled.mode_changed);

        assert_eq!(
            evaluate(&mut engine, 109, ActivityLedMode::Enabled).level,
            OutputLevel::Low
        );
        let idle = evaluate(&mut engine, 110, ActivityLedMode::Enabled);
        assert_eq!(idle.level, OutputLevel::High);
        assert_eq!(idle.period_position_ms, Some(100));
        assert_eq!(
            evaluate(&mut engine, 2_009, ActivityLedMode::Enabled).level,
            OutputLevel::High
        );
        let next = evaluate(&mut engine, 2_010, ActivityLedMode::Enabled);
        assert_eq!(next.level, OutputLevel::Low);
        assert_eq!(next.period_position_ms, Some(0));
    }

    #[test]
    fn late_evaluation_uses_absolute_phase_and_never_accumulates_drift() {
        let mut engine = ActivityLedPolicyEngine::new(vendor_shaped_config(), 1_000).unwrap();
        let _ = evaluate(&mut engine, 1_000, ActivityLedMode::Enabled);
        let late = evaluate(&mut engine, 123_456, ActivityLedMode::Enabled);
        assert_eq!(late.period_position_ms, Some(456));
        assert_eq!(late.level, OutputLevel::High);
        let much_later = evaluate(&mut engine, 1_001_050, ActivityLedMode::Enabled);
        assert_eq!(much_later.period_position_ms, Some(50));
        assert_eq!(much_later.level, OutputLevel::Low);
    }

    #[test]
    fn disabled_and_fault_are_inactive_and_reenable_restarts_pulse() {
        let mut engine = ActivityLedPolicyEngine::new(vendor_shaped_config(), 0).unwrap();
        let _ = evaluate(&mut engine, 0, ActivityLedMode::Enabled);
        let disabled = evaluate(&mut engine, 50, ActivityLedMode::Disabled);
        assert_eq!(disabled.level, OutputLevel::High);
        assert_eq!(disabled.reason, ActivityLedReason::DisabledInactive);
        assert_eq!(disabled.period_position_ms, None);
        let fault = evaluate(&mut engine, 60, ActivityLedMode::Fault);
        assert_eq!(fault.level, OutputLevel::High);
        assert_eq!(fault.reason, ActivityLedReason::FaultInactive);
        let reenabled = evaluate(&mut engine, 70, ActivityLedMode::Enabled);
        assert_eq!(reenabled.level, OutputLevel::Low);
        assert_eq!(reenabled.period_position_ms, Some(0));
        assert_eq!(reenabled.state.enabled_at_ms, Some(70));
    }

    #[test]
    fn active_high_configuration_inverts_only_the_electrical_level() {
        let config = ActivityLedConfig {
            asserted_level: OutputLevel::High,
            period_ms: 1_000,
            pulse_ms: 100,
        };
        let mut engine = ActivityLedPolicyEngine::new(config, 0).unwrap();
        assert_eq!(engine.state().level, OutputLevel::Low);
        assert_eq!(
            evaluate(&mut engine, 0, ActivityLedMode::Enabled).level,
            OutputLevel::High
        );
        assert_eq!(
            evaluate(&mut engine, 100, ActivityLedMode::Enabled).level,
            OutputLevel::Low
        );
    }

    #[test]
    fn clock_rollback_is_rejected_without_state_mutation() {
        let mut engine = ActivityLedPolicyEngine::new(vendor_shaped_config(), 100).unwrap();
        let _ = evaluate(&mut engine, 200, ActivityLedMode::Enabled);
        let before = engine.state();
        assert_eq!(
            engine.evaluate(199, ActivityLedMode::Fault),
            Err(ActivityLedPolicyError::ClockMovedBackwards)
        );
        assert_eq!(engine.state(), before);
    }

    #[test]
    fn restart_always_returns_to_disabled_inactive_state() {
        let mut first = ActivityLedPolicyEngine::new(vendor_shaped_config(), 0).unwrap();
        let _ = evaluate(&mut first, 0, ActivityLedMode::Enabled);
        assert_eq!(first.state().level, OutputLevel::Low);
        let restarted = ActivityLedPolicyEngine::new(vendor_shaped_config(), 5).unwrap();
        assert_eq!(restarted.state().mode, ActivityLedMode::Disabled);
        assert_eq!(restarted.state().level, OutputLevel::High);
        assert_eq!(restarted.state().enabled_at_ms, None);
    }

    #[test]
    fn exhaustive_two_period_schedule_matches_explicit_polarity() {
        for asserted_level in [OutputLevel::Low, OutputLevel::High] {
            let config = ActivityLedConfig {
                asserted_level,
                period_ms: 137,
                pulse_ms: 43,
            };
            let mut engine = ActivityLedPolicyEngine::new(config, 1_000).unwrap();
            for offset in 0..(config.period_ms * 2) {
                let decision = evaluate(&mut engine, 1_000 + offset, ActivityLedMode::Enabled);
                let expected = if offset % config.period_ms < config.pulse_ms {
                    asserted_level
                } else {
                    asserted_level.opposite()
                };
                assert_eq!(decision.level, expected, "offset {offset}");
            }
        }
    }
}
