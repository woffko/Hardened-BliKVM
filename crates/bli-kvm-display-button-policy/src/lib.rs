#![no_std]
#![forbid(unsafe_code)]

//! Pure display-button debounce and one-shot gesture policy.
//!
//! Callers supply monotonic time and raw high/low observations. The policy has
//! no GPIO, display, filesystem, thread, clock, or device access. A wake intent
//! is emitted at most once per debounced release-to-press cycle.

pub const MIN_DEBOUNCE_MS: u64 = 5;
pub const MAX_DEBOUNCE_MS: u64 = 1_000;
pub const MIN_ACTIVATION_HOLD_MS: u64 = 50;
pub const MAX_ACTIVATION_HOLD_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ButtonLevel {
    Low,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonPolicyConfig {
    pub active_level: ButtonLevel,
    pub debounce_ms: u64,
    pub activation_hold_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonConfigError {
    DebounceOutOfRange,
    ActivationHoldOutOfRange,
    ActivationHoldShorterThanDebounce,
}

impl ButtonPolicyConfig {
    pub const fn validate(self) -> Result<(), ButtonConfigError> {
        if self.debounce_ms < MIN_DEBOUNCE_MS || self.debounce_ms > MAX_DEBOUNCE_MS {
            return Err(ButtonConfigError::DebounceOutOfRange);
        }
        if self.activation_hold_ms < MIN_ACTIVATION_HOLD_MS
            || self.activation_hold_ms > MAX_ACTIVATION_HOLD_MS
        {
            return Err(ButtonConfigError::ActivationHoldOutOfRange);
        }
        if self.activation_hold_ms < self.debounce_ms {
            return Err(ButtonConfigError::ActivationHoldShorterThanDebounce);
        }
        Ok(())
    }

    pub const fn is_active(self, level: ButtonLevel) -> bool {
        matches!(
            (self.active_level, level),
            (ButtonLevel::Low, ButtonLevel::Low) | (ButtonLevel::High, ButtonLevel::High)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayButtonEvent {
    WakeDisplayCycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonReason {
    StartupUnsettled,
    RawTransition,
    Debouncing,
    ReleasedAndArmed,
    ReleasedIdle,
    PressDebounced,
    PressHolding,
    WakeCycleRequested,
    StartupHeldUnarmed,
    HeldLatchedUntilRelease,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonPolicyError {
    ClockMovedBackwards,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonPolicyState {
    pub raw_level: ButtonLevel,
    pub stable_level: Option<ButtonLevel>,
    pub raw_changed_at_ms: u64,
    pub stable_changed_at_ms: Option<u64>,
    pub press_started_at_ms: Option<u64>,
    pub last_evaluation_ms: u64,
    pub armed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonOutcome {
    pub event: Option<DisplayButtonEvent>,
    pub reason: ButtonReason,
    pub state: ButtonPolicyState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonPolicyEngine {
    config: ButtonPolicyConfig,
    raw_level: ButtonLevel,
    stable_level: Option<ButtonLevel>,
    raw_changed_at_ms: u64,
    stable_changed_at_ms: Option<u64>,
    press_started_at_ms: Option<u64>,
    last_evaluation_ms: u64,
    armed: bool,
}

impl ButtonPolicyEngine {
    pub fn new(
        config: ButtonPolicyConfig,
        started_at_ms: u64,
        initial_level: ButtonLevel,
    ) -> Result<Self, ButtonConfigError> {
        config.validate()?;
        Ok(Self {
            config,
            raw_level: initial_level,
            stable_level: None,
            raw_changed_at_ms: started_at_ms,
            stable_changed_at_ms: None,
            press_started_at_ms: None,
            last_evaluation_ms: started_at_ms,
            armed: false,
        })
    }

    pub const fn state(&self) -> ButtonPolicyState {
        ButtonPolicyState {
            raw_level: self.raw_level,
            stable_level: self.stable_level,
            raw_changed_at_ms: self.raw_changed_at_ms,
            stable_changed_at_ms: self.stable_changed_at_ms,
            press_started_at_ms: self.press_started_at_ms,
            last_evaluation_ms: self.last_evaluation_ms,
            armed: self.armed,
        }
    }

    pub fn evaluate(
        &mut self,
        now_ms: u64,
        level: ButtonLevel,
    ) -> Result<ButtonOutcome, ButtonPolicyError> {
        if now_ms < self.last_evaluation_ms {
            return Err(ButtonPolicyError::ClockMovedBackwards);
        }

        let mut event = None;
        let reason;
        if level != self.raw_level {
            self.raw_level = level;
            self.raw_changed_at_ms = now_ms;
            reason = ButtonReason::RawTransition;
        } else if self.stable_level != Some(self.raw_level) {
            if now_ms.saturating_sub(self.raw_changed_at_ms) < self.config.debounce_ms {
                reason = if self.stable_level.is_none() {
                    ButtonReason::StartupUnsettled
                } else {
                    ButtonReason::Debouncing
                };
            } else {
                self.stable_level = Some(self.raw_level);
                self.stable_changed_at_ms = Some(now_ms);
                if self.config.is_active(self.raw_level) {
                    if self.armed {
                        self.press_started_at_ms = Some(now_ms);
                        reason = ButtonReason::PressDebounced;
                    } else {
                        self.press_started_at_ms = None;
                        reason = ButtonReason::StartupHeldUnarmed;
                    }
                } else {
                    self.armed = true;
                    self.press_started_at_ms = None;
                    reason = ButtonReason::ReleasedAndArmed;
                }
            }
        } else if self.config.is_active(self.raw_level) {
            if !self.armed {
                reason = ButtonReason::HeldLatchedUntilRelease;
            } else if let Some(press_started_at_ms) = self.press_started_at_ms {
                if now_ms.saturating_sub(press_started_at_ms) >= self.config.activation_hold_ms {
                    event = Some(DisplayButtonEvent::WakeDisplayCycle);
                    self.armed = false;
                    self.press_started_at_ms = None;
                    reason = ButtonReason::WakeCycleRequested;
                } else {
                    reason = ButtonReason::PressHolding;
                }
            } else {
                self.press_started_at_ms = Some(now_ms);
                reason = ButtonReason::PressDebounced;
            }
        } else if self.armed {
            reason = ButtonReason::ReleasedIdle;
        } else {
            self.armed = true;
            self.press_started_at_ms = None;
            reason = ButtonReason::ReleasedAndArmed;
        }

        self.last_evaluation_ms = now_ms;
        Ok(ButtonOutcome {
            event,
            reason,
            state: self.state(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(active_level: ButtonLevel) -> ButtonPolicyConfig {
        ButtonPolicyConfig {
            active_level,
            debounce_ms: 20,
            activation_hold_ms: 100,
        }
    }

    fn evaluate(engine: &mut ButtonPolicyEngine, now_ms: u64, level: ButtonLevel) -> ButtonOutcome {
        engine.evaluate(now_ms, level).unwrap()
    }

    #[test]
    fn configuration_requires_explicit_bounded_timing() {
        assert_eq!(
            ButtonPolicyConfig {
                active_level: ButtonLevel::High,
                debounce_ms: 4,
                activation_hold_ms: 100,
            }
            .validate(),
            Err(ButtonConfigError::DebounceOutOfRange)
        );
        assert_eq!(
            ButtonPolicyConfig {
                active_level: ButtonLevel::High,
                debounce_ms: 20,
                activation_hold_ms: 49,
            }
            .validate(),
            Err(ButtonConfigError::ActivationHoldOutOfRange)
        );
        assert_eq!(
            ButtonPolicyConfig {
                active_level: ButtonLevel::High,
                debounce_ms: 100,
                activation_hold_ms: 50,
            }
            .validate(),
            Err(ButtonConfigError::ActivationHoldShorterThanDebounce)
        );
        assert!(
            ButtonPolicyConfig {
                active_level: ButtonLevel::Low,
                debounce_ms: MAX_DEBOUNCE_MS,
                activation_hold_ms: MAX_ACTIVATION_HOLD_MS,
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn one_event_is_emitted_per_debounced_press_release_cycle() {
        let mut engine =
            ButtonPolicyEngine::new(config(ButtonLevel::High), 0, ButtonLevel::Low).unwrap();
        assert_eq!(
            evaluate(&mut engine, 19, ButtonLevel::Low).reason,
            ButtonReason::StartupUnsettled
        );
        let armed = evaluate(&mut engine, 20, ButtonLevel::Low);
        assert_eq!(armed.reason, ButtonReason::ReleasedAndArmed);
        assert!(armed.state.armed);

        assert_eq!(
            evaluate(&mut engine, 30, ButtonLevel::High).reason,
            ButtonReason::RawTransition
        );
        assert_eq!(
            evaluate(&mut engine, 49, ButtonLevel::High).reason,
            ButtonReason::Debouncing
        );
        let pressed = evaluate(&mut engine, 50, ButtonLevel::High);
        assert_eq!(pressed.reason, ButtonReason::PressDebounced);
        assert_eq!(pressed.event, None);
        assert_eq!(
            evaluate(&mut engine, 149, ButtonLevel::High).reason,
            ButtonReason::PressHolding
        );
        let wake = evaluate(&mut engine, 150, ButtonLevel::High);
        assert_eq!(wake.event, Some(DisplayButtonEvent::WakeDisplayCycle));
        assert_eq!(wake.reason, ButtonReason::WakeCycleRequested);

        for now in [151, 650, 10_000] {
            let held = evaluate(&mut engine, now, ButtonLevel::High);
            assert_eq!(held.event, None);
            assert_eq!(held.reason, ButtonReason::HeldLatchedUntilRelease);
        }

        assert_eq!(
            evaluate(&mut engine, 10_010, ButtonLevel::Low).reason,
            ButtonReason::RawTransition
        );
        let released = evaluate(&mut engine, 10_030, ButtonLevel::Low);
        assert_eq!(released.reason, ButtonReason::ReleasedAndArmed);
        assert!(released.state.armed);

        let _ = evaluate(&mut engine, 10_040, ButtonLevel::High);
        let _ = evaluate(&mut engine, 10_060, ButtonLevel::High);
        let second = evaluate(&mut engine, 10_160, ButtonLevel::High);
        assert_eq!(second.event, Some(DisplayButtonEvent::WakeDisplayCycle));
    }

    #[test]
    fn startup_held_input_never_fires_until_release_and_new_press() {
        let mut engine =
            ButtonPolicyEngine::new(config(ButtonLevel::High), 0, ButtonLevel::High).unwrap();
        let initial = evaluate(&mut engine, 20, ButtonLevel::High);
        assert_eq!(initial.reason, ButtonReason::StartupHeldUnarmed);
        assert!(!initial.state.armed);
        for now in [120, 1_000, 100_000] {
            assert_eq!(evaluate(&mut engine, now, ButtonLevel::High).event, None);
        }
        let _ = evaluate(&mut engine, 100_010, ButtonLevel::Low);
        let released = evaluate(&mut engine, 100_030, ButtonLevel::Low);
        assert!(released.state.armed);
        let _ = evaluate(&mut engine, 100_040, ButtonLevel::High);
        let _ = evaluate(&mut engine, 100_060, ButtonLevel::High);
        assert_eq!(
            evaluate(&mut engine, 100_160, ButtonLevel::High).event,
            Some(DisplayButtonEvent::WakeDisplayCycle)
        );
    }

    #[test]
    fn bounce_never_arms_or_fires_an_uncertain_level() {
        let mut engine =
            ButtonPolicyEngine::new(config(ButtonLevel::High), 0, ButtonLevel::Low).unwrap();
        let _ = evaluate(&mut engine, 20, ButtonLevel::Low);
        let _ = evaluate(&mut engine, 30, ButtonLevel::High);
        let _ = evaluate(&mut engine, 40, ButtonLevel::Low);
        let _ = evaluate(&mut engine, 45, ButtonLevel::High);
        assert_eq!(evaluate(&mut engine, 64, ButtonLevel::High).event, None);
        let pressed = evaluate(&mut engine, 65, ButtonLevel::High);
        assert_eq!(pressed.reason, ButtonReason::PressDebounced);
        assert_eq!(evaluate(&mut engine, 164, ButtonLevel::High).event, None);
        assert_eq!(
            evaluate(&mut engine, 165, ButtonLevel::High).event,
            Some(DisplayButtonEvent::WakeDisplayCycle)
        );

        let _ = evaluate(&mut engine, 200, ButtonLevel::Low);
        let _ = evaluate(&mut engine, 210, ButtonLevel::High);
        assert!(!engine.state().armed);
        assert_eq!(evaluate(&mut engine, 1_000, ButtonLevel::High).event, None);
    }

    #[test]
    fn active_low_configuration_has_the_same_one_shot_semantics() {
        let mut engine =
            ButtonPolicyEngine::new(config(ButtonLevel::Low), 0, ButtonLevel::High).unwrap();
        let _ = evaluate(&mut engine, 20, ButtonLevel::High);
        let _ = evaluate(&mut engine, 30, ButtonLevel::Low);
        let _ = evaluate(&mut engine, 50, ButtonLevel::Low);
        assert_eq!(
            evaluate(&mut engine, 150, ButtonLevel::Low).event,
            Some(DisplayButtonEvent::WakeDisplayCycle)
        );
        assert_eq!(evaluate(&mut engine, 1_000, ButtonLevel::Low).event, None);
    }

    #[test]
    fn clock_rollback_is_rejected_without_state_mutation() {
        let mut engine =
            ButtonPolicyEngine::new(config(ButtonLevel::High), 100, ButtonLevel::Low).unwrap();
        let _ = evaluate(&mut engine, 120, ButtonLevel::Low);
        let before = engine.state();
        assert_eq!(
            engine.evaluate(119, ButtonLevel::High),
            Err(ButtonPolicyError::ClockMovedBackwards)
        );
        assert_eq!(engine.state(), before);
    }

    #[test]
    fn bounded_binary_sample_patterns_never_repeat_without_release() {
        let exhaustive_config = ButtonPolicyConfig {
            active_level: ButtonLevel::High,
            debounce_ms: 10,
            activation_hold_ms: 50,
        };
        for pattern in 0_u16..(1_u16 << 12) {
            let mut engine =
                ButtonPolicyEngine::new(exhaustive_config, 0, ButtonLevel::Low).unwrap();
            let mut event_latched = false;
            for step in 0..12 {
                let level = if pattern & (1 << step) == 0 {
                    ButtonLevel::Low
                } else {
                    ButtonLevel::High
                };
                let outcome = evaluate(&mut engine, (step as u64 + 1) * 10, level);
                if outcome.state.stable_level == Some(ButtonLevel::Low) {
                    event_latched = false;
                }
                if outcome.event.is_some() {
                    assert!(
                        !event_latched,
                        "pattern {pattern:#05x} repeated without release"
                    );
                    event_latched = true;
                }
            }
        }
    }
}
