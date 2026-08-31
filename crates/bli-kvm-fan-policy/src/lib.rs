//! Pure fan-policy state machine with no GPIO, PWM, filesystem, or clock access.
//!
//! Callers supply monotonic milliseconds and sanitized temperature observations.
//! The returned command is a policy result only; this crate cannot actuate hardware.

use thiserror::Error;

pub const MIN_TEMPERATURE_MILLIDEGREES: i32 = -40_000;
pub const MAX_TEMPERATURE_MILLIDEGREES: i32 = 150_000;
pub const FAILSAFE_DUTY_PERCENT: u8 = 100;

const MIN_TIMING_MS: u64 = 1_000;
const MAX_DWELL_MS: u64 = 600_000;
const MAX_SENSOR_TIMEOUT_MS: u64 = 300_000;
const MAX_CURVE_POINTS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanCurvePoint {
    pub at_or_above_millidegrees: i32,
    pub duty_percent: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanPolicyConfig {
    pub turn_off_millidegrees: i32,
    pub turn_on_millidegrees: i32,
    pub critical_millidegrees: i32,
    pub minimum_on_ms: u64,
    pub minimum_off_ms: u64,
    pub sensor_timeout_ms: u64,
    pub minimum_duty_percent: u8,
    pub curve: Vec<FanCurvePoint>,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum FanConfigError {
    #[error("fan temperature thresholds are invalid")]
    InvalidThresholds,
    #[error("fan minimum on/off timing is invalid")]
    InvalidDwell,
    #[error("fan sensor timeout is invalid")]
    InvalidSensorTimeout,
    #[error("fan minimum duty is invalid")]
    InvalidMinimumDuty,
    #[error("fan duty curve is invalid")]
    InvalidCurve,
}

impl FanPolicyConfig {
    pub fn validate(&self) -> Result<(), FanConfigError> {
        if !(MIN_TEMPERATURE_MILLIDEGREES..=MAX_TEMPERATURE_MILLIDEGREES)
            .contains(&self.turn_off_millidegrees)
            || !(MIN_TEMPERATURE_MILLIDEGREES..=MAX_TEMPERATURE_MILLIDEGREES)
                .contains(&self.turn_on_millidegrees)
            || !(MIN_TEMPERATURE_MILLIDEGREES..=MAX_TEMPERATURE_MILLIDEGREES)
                .contains(&self.critical_millidegrees)
            || self.turn_off_millidegrees >= self.turn_on_millidegrees
            || self.turn_on_millidegrees >= self.critical_millidegrees
        {
            return Err(FanConfigError::InvalidThresholds);
        }
        if !(MIN_TIMING_MS..=MAX_DWELL_MS).contains(&self.minimum_on_ms)
            || !(MIN_TIMING_MS..=MAX_DWELL_MS).contains(&self.minimum_off_ms)
        {
            return Err(FanConfigError::InvalidDwell);
        }
        if !(MIN_TIMING_MS..=MAX_SENSOR_TIMEOUT_MS).contains(&self.sensor_timeout_ms) {
            return Err(FanConfigError::InvalidSensorTimeout);
        }
        if !(1..=100).contains(&self.minimum_duty_percent) {
            return Err(FanConfigError::InvalidMinimumDuty);
        }
        if self.curve.is_empty() || self.curve.len() > MAX_CURVE_POINTS {
            return Err(FanConfigError::InvalidCurve);
        }
        let mut previous_temperature = None;
        let mut previous_duty = self.minimum_duty_percent;
        for point in &self.curve {
            if !(MIN_TEMPERATURE_MILLIDEGREES..=MAX_TEMPERATURE_MILLIDEGREES)
                .contains(&point.at_or_above_millidegrees)
                || !(self.minimum_duty_percent..=100).contains(&point.duty_percent)
                || previous_temperature
                    .is_some_and(|previous| point.at_or_above_millidegrees <= previous)
                || point.duty_percent < previous_duty
            {
                return Err(FanConfigError::InvalidCurve);
            }
            previous_temperature = Some(point.at_or_above_millidegrees);
            previous_duty = point.duty_percent;
        }
        Ok(())
    }

    fn duty_for(&self, temperature_millidegrees: i32) -> u8 {
        self.curve
            .iter()
            .take_while(|point| temperature_millidegrees >= point.at_or_above_millidegrees)
            .last()
            .map_or(self.minimum_duty_percent, |point| point.duty_percent)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanObservation {
    TemperatureMillidegrees(i32),
    SensorUnavailable,
    NoNewSample,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanDutyPercent(u8);

impl FanDutyPercent {
    pub const MAXIMUM: Self = Self(100);

    pub const fn new(value: u8) -> Option<Self> {
        if value >= 1 && value <= 100 {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanCommand {
    Off,
    DutyPercent(FanDutyPercent),
}

impl FanCommand {
    pub const fn is_powered(self) -> bool {
        matches!(self, Self::DutyPercent(_))
    }

    pub const fn duty_percent(self) -> Option<u8> {
        match self {
            Self::Off => None,
            Self::DutyPercent(duty) => Some(duty.get()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanMode {
    Off,
    Running,
    Failsafe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanReason {
    StartupFailsafe,
    SensorUnavailable,
    SensorInvalid,
    SensorTimeout,
    AwaitingSample,
    CriticalTemperature,
    AboveOnThreshold,
    BelowOnThreshold,
    BelowOffThreshold,
    HysteresisHold,
    MinimumOnTime,
    MinimumOffTime,
    SensorRecovered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanDecision {
    pub command: FanCommand,
    pub mode: FanMode,
    pub reason: FanReason,
    pub state_changed: bool,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum FanPolicyError {
    #[error("monotonic time moved backwards")]
    ClockMovedBackwards,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanPolicyState {
    pub command: FanCommand,
    pub mode: FanMode,
    pub reason: FanReason,
    pub last_power_transition_ms: u64,
    pub last_evaluation_ms: u64,
    pub last_valid_sample_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct FanPolicyEngine {
    config: FanPolicyConfig,
    command: FanCommand,
    failsafe_active: bool,
    reason: FanReason,
    last_power_transition_ms: u64,
    last_evaluation_ms: u64,
    last_valid_sample_ms: Option<u64>,
}

impl FanPolicyEngine {
    pub fn new(config: FanPolicyConfig, started_at_ms: u64) -> Result<Self, FanConfigError> {
        config.validate()?;
        Ok(Self {
            config,
            command: FanCommand::DutyPercent(FanDutyPercent(FAILSAFE_DUTY_PERCENT)),
            failsafe_active: true,
            reason: FanReason::StartupFailsafe,
            last_power_transition_ms: started_at_ms,
            last_evaluation_ms: started_at_ms,
            last_valid_sample_ms: None,
        })
    }

    pub fn state(&self) -> FanPolicyState {
        FanPolicyState {
            command: self.command,
            mode: self.mode(),
            reason: self.reason,
            last_power_transition_ms: self.last_power_transition_ms,
            last_evaluation_ms: self.last_evaluation_ms,
            last_valid_sample_ms: self.last_valid_sample_ms,
        }
    }

    pub fn evaluate(
        &mut self,
        now_ms: u64,
        observation: FanObservation,
    ) -> Result<FanDecision, FanPolicyError> {
        if now_ms < self.last_evaluation_ms {
            return Err(FanPolicyError::ClockMovedBackwards);
        }
        let previous_command = self.command;
        let previous_mode = self.mode();
        let reason = match observation {
            FanObservation::SensorUnavailable => {
                self.enter_failsafe(now_ms);
                FanReason::SensorUnavailable
            }
            FanObservation::NoNewSample => {
                if self.failsafe_active
                    || self.last_valid_sample_ms.is_none_or(|last| {
                        now_ms.saturating_sub(last) >= self.config.sensor_timeout_ms
                    })
                {
                    self.enter_failsafe(now_ms);
                    FanReason::SensorTimeout
                } else {
                    FanReason::AwaitingSample
                }
            }
            FanObservation::TemperatureMillidegrees(temperature)
                if !(MIN_TEMPERATURE_MILLIDEGREES..=MAX_TEMPERATURE_MILLIDEGREES)
                    .contains(&temperature) =>
            {
                self.enter_failsafe(now_ms);
                FanReason::SensorInvalid
            }
            FanObservation::TemperatureMillidegrees(temperature) => {
                self.last_valid_sample_ms = Some(now_ms);
                if temperature >= self.config.critical_millidegrees {
                    self.enter_failsafe(now_ms);
                    FanReason::CriticalTemperature
                } else {
                    self.evaluate_valid_temperature(now_ms, temperature)
                }
            }
        };
        self.reason = reason;
        self.last_evaluation_ms = now_ms;
        let after = self.state();
        Ok(FanDecision {
            command: after.command,
            mode: after.mode,
            reason,
            state_changed: previous_command != after.command || previous_mode != after.mode,
        })
    }

    fn evaluate_valid_temperature(&mut self, now_ms: u64, temperature: i32) -> FanReason {
        let was_failsafe = self.failsafe_active;
        self.failsafe_active = false;
        match self.command {
            FanCommand::Off => {
                if temperature >= self.config.turn_on_millidegrees {
                    if self.power_transition_elapsed(now_ms) >= self.config.minimum_off_ms {
                        self.set_command(
                            now_ms,
                            FanCommand::DutyPercent(FanDutyPercent(
                                self.config.duty_for(temperature),
                            )),
                        );
                        if was_failsafe {
                            FanReason::SensorRecovered
                        } else {
                            FanReason::AboveOnThreshold
                        }
                    } else {
                        FanReason::MinimumOffTime
                    }
                } else {
                    FanReason::BelowOnThreshold
                }
            }
            FanCommand::DutyPercent(_) => {
                if temperature <= self.config.turn_off_millidegrees {
                    if self.power_transition_elapsed(now_ms) >= self.config.minimum_on_ms {
                        self.set_command(now_ms, FanCommand::Off);
                        FanReason::BelowOffThreshold
                    } else {
                        self.command = FanCommand::DutyPercent(FanDutyPercent(
                            self.config.duty_for(temperature),
                        ));
                        FanReason::MinimumOnTime
                    }
                } else {
                    self.command =
                        FanCommand::DutyPercent(FanDutyPercent(self.config.duty_for(temperature)));
                    if was_failsafe {
                        FanReason::SensorRecovered
                    } else {
                        FanReason::HysteresisHold
                    }
                }
            }
        }
    }

    fn enter_failsafe(&mut self, now_ms: u64) {
        self.failsafe_active = true;
        self.set_command(
            now_ms,
            FanCommand::DutyPercent(FanDutyPercent(FAILSAFE_DUTY_PERCENT)),
        );
    }

    fn set_command(&mut self, now_ms: u64, command: FanCommand) {
        if self.command.is_powered() != command.is_powered() {
            self.last_power_transition_ms = now_ms;
        }
        self.command = command;
    }

    fn power_transition_elapsed(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.last_power_transition_ms)
    }

    fn mode(&self) -> FanMode {
        if self.failsafe_active {
            FanMode::Failsafe
        } else if self.command.is_powered() {
            FanMode::Running
        } else {
            FanMode::Off
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn duty(value: u8) -> FanCommand {
        FanCommand::DutyPercent(FanDutyPercent::new(value).unwrap())
    }

    fn config() -> FanPolicyConfig {
        FanPolicyConfig {
            turn_off_millidegrees: 60_000,
            turn_on_millidegrees: 65_000,
            critical_millidegrees: 85_000,
            minimum_on_ms: 30_000,
            minimum_off_ms: 10_000,
            sensor_timeout_ms: 20_000,
            minimum_duty_percent: 50,
            curve: vec![
                FanCurvePoint {
                    at_or_above_millidegrees: 45_000,
                    duty_percent: 60,
                },
                FanCurvePoint {
                    at_or_above_millidegrees: 50_000,
                    duty_percent: 70,
                },
                FanCurvePoint {
                    at_or_above_millidegrees: 53_000,
                    duty_percent: 80,
                },
                FanCurvePoint {
                    at_or_above_millidegrees: 55_000,
                    duty_percent: 90,
                },
                FanCurvePoint {
                    at_or_above_millidegrees: 60_000,
                    duty_percent: 100,
                },
            ],
        }
    }

    #[test]
    fn startup_is_maximum_cooling_and_hysteresis_respects_dwell() {
        let mut engine = FanPolicyEngine::new(config(), 0).unwrap();
        assert_eq!(
            engine.state(),
            FanPolicyState {
                command: duty(100),
                mode: FanMode::Failsafe,
                reason: FanReason::StartupFailsafe,
                last_power_transition_ms: 0,
                last_evaluation_ms: 0,
                last_valid_sample_ms: None,
            }
        );

        let decision = engine
            .evaluate(0, FanObservation::TemperatureMillidegrees(50_000))
            .unwrap();
        assert_eq!(decision.command, duty(70));
        assert_eq!(decision.reason, FanReason::MinimumOnTime);
        assert_eq!(decision.mode, FanMode::Running);

        let decision = engine
            .evaluate(30_000, FanObservation::TemperatureMillidegrees(50_000))
            .unwrap();
        assert_eq!(decision.command, FanCommand::Off);
        assert_eq!(decision.reason, FanReason::BelowOffThreshold);

        let decision = engine
            .evaluate(35_000, FanObservation::TemperatureMillidegrees(70_000))
            .unwrap();
        assert_eq!(decision.command, FanCommand::Off);
        assert_eq!(decision.reason, FanReason::MinimumOffTime);

        let decision = engine
            .evaluate(40_000, FanObservation::TemperatureMillidegrees(70_000))
            .unwrap();
        assert_eq!(decision.command, duty(100));
        assert_eq!(decision.reason, FanReason::AboveOnThreshold);

        let decision = engine
            .evaluate(50_000, FanObservation::TemperatureMillidegrees(62_000))
            .unwrap();
        assert_eq!(decision.command, duty(100));
        assert_eq!(decision.reason, FanReason::HysteresisHold);

        let decision = engine
            .evaluate(70_000, FanObservation::TemperatureMillidegrees(60_000))
            .unwrap();
        assert_eq!(decision.command, FanCommand::Off);
    }

    #[test]
    fn faults_invalid_input_and_critical_heat_override_dwell() {
        for observation in [
            FanObservation::SensorUnavailable,
            FanObservation::TemperatureMillidegrees(MAX_TEMPERATURE_MILLIDEGREES + 1),
            FanObservation::TemperatureMillidegrees(85_000),
        ] {
            let mut engine = FanPolicyEngine::new(config(), 0).unwrap();
            let _ = engine
                .evaluate(30_000, FanObservation::TemperatureMillidegrees(50_000))
                .unwrap();
            assert_eq!(engine.state().command, FanCommand::Off);
            let decision = engine.evaluate(31_000, observation).unwrap();
            assert_eq!(decision.command, duty(100));
            assert_eq!(decision.mode, FanMode::Failsafe);
        }
    }

    #[test]
    fn missing_samples_fail_safe_at_the_exact_timeout() {
        let mut engine = FanPolicyEngine::new(config(), 0).unwrap();
        let _ = engine
            .evaluate(0, FanObservation::TemperatureMillidegrees(70_000))
            .unwrap();
        let before = engine
            .evaluate(19_999, FanObservation::NoNewSample)
            .unwrap();
        assert_eq!(before.mode, FanMode::Running);
        assert_eq!(before.reason, FanReason::AwaitingSample);
        assert!(!before.state_changed);
        let expired = engine
            .evaluate(20_000, FanObservation::NoNewSample)
            .unwrap();
        assert_eq!(expired.mode, FanMode::Failsafe);
        assert_eq!(expired.command, duty(100));
        assert_eq!(expired.reason, FanReason::SensorTimeout);
    }

    #[test]
    fn duty_curve_is_monotonic_and_every_output_is_bounded() {
        let policy = config();
        let expected = [
            (44_999, 50),
            (45_000, 60),
            (50_000, 70),
            (53_000, 80),
            (55_000, 90),
            (60_000, 100),
            (84_999, 100),
        ];
        for (temperature, duty) in expected {
            assert_eq!(policy.duty_for(temperature), duty);
        }

        let mut previous = 0;
        for temperature in
            (MIN_TEMPERATURE_MILLIDEGREES..=MAX_TEMPERATURE_MILLIDEGREES).step_by(500)
        {
            let duty = policy.duty_for(temperature);
            assert!((1..=100).contains(&duty));
            assert!(duty >= previous);
            previous = duty;
        }
    }

    #[test]
    fn all_reachable_modes_keep_commands_bounded_and_faults_at_maximum() {
        let startup = FanPolicyEngine::new(config(), 0).unwrap();
        let mut off = startup.clone();
        let _ = off
            .evaluate(30_000, FanObservation::TemperatureMillidegrees(50_000))
            .unwrap();
        assert_eq!(off.state().mode, FanMode::Off);
        let mut running = off.clone();
        let _ = running
            .evaluate(40_000, FanObservation::TemperatureMillidegrees(70_000))
            .unwrap();
        assert_eq!(running.state().mode, FanMode::Running);

        for base in [startup, off, running] {
            for temperature in [
                -40_001, -40_000, 0, 59_999, 60_000, 62_000, 65_000, 84_999, 85_000, 150_000,
                150_001,
            ] {
                let mut engine = base.clone();
                let decision = engine
                    .evaluate(
                        engine.state().last_evaluation_ms + 1,
                        FanObservation::TemperatureMillidegrees(temperature),
                    )
                    .unwrap();
                match (decision.mode, decision.command) {
                    (FanMode::Off, FanCommand::Off) => {}
                    (FanMode::Running, FanCommand::DutyPercent(duty)) => {
                        assert!((1..=100).contains(&duty.get()));
                    }
                    (FanMode::Failsafe, FanCommand::DutyPercent(duty)) if duty.get() == 100 => {}
                    pair => panic!("invalid fan mode/command pair: {pair:?}"),
                }
                if !(MIN_TEMPERATURE_MILLIDEGREES..=MAX_TEMPERATURE_MILLIDEGREES)
                    .contains(&temperature)
                    || temperature >= config().critical_millidegrees
                {
                    assert_eq!(decision.mode, FanMode::Failsafe);
                    assert_eq!(decision.command, duty(100));
                }
            }

            let mut engine = base;
            let decision = engine
                .evaluate(
                    engine.state().last_evaluation_ms + 1,
                    FanObservation::SensorUnavailable,
                )
                .unwrap();
            assert_eq!(decision.mode, FanMode::Failsafe);
            assert_eq!(decision.command, duty(100));
        }
    }

    #[test]
    fn invalid_configs_are_rejected() {
        let mut invalid = config();
        invalid.turn_off_millidegrees = invalid.turn_on_millidegrees;
        assert_eq!(invalid.validate(), Err(FanConfigError::InvalidThresholds));

        let mut invalid = config();
        invalid.minimum_on_ms = 0;
        assert_eq!(invalid.validate(), Err(FanConfigError::InvalidDwell));

        let mut invalid = config();
        invalid.sensor_timeout_ms = MAX_SENSOR_TIMEOUT_MS + 1;
        assert_eq!(
            invalid.validate(),
            Err(FanConfigError::InvalidSensorTimeout)
        );

        let mut invalid = config();
        invalid.minimum_duty_percent = 0;
        assert_eq!(invalid.validate(), Err(FanConfigError::InvalidMinimumDuty));

        let mut invalid = config();
        invalid.curve[1].duty_percent = 40;
        assert_eq!(invalid.validate(), Err(FanConfigError::InvalidCurve));

        let mut invalid = config();
        invalid.curve[1].at_or_above_millidegrees = invalid.curve[0].at_or_above_millidegrees;
        assert_eq!(invalid.validate(), Err(FanConfigError::InvalidCurve));
    }

    #[test]
    fn duty_type_cannot_represent_zero_or_above_one_hundred() {
        assert_eq!(FanDutyPercent::new(0), None);
        assert_eq!(FanDutyPercent::new(101), None);
        assert_eq!(FanDutyPercent::new(1).map(FanDutyPercent::get), Some(1));
        assert_eq!(FanDutyPercent::new(100).map(FanDutyPercent::get), Some(100));
        assert_eq!(duty(75).duty_percent(), Some(75));
        assert_eq!(FanCommand::Off.duty_percent(), None);
    }

    #[test]
    fn monotonic_clock_rollback_is_rejected_without_state_change() {
        let mut engine = FanPolicyEngine::new(config(), 1_000).unwrap();
        let _ = engine
            .evaluate(2_000, FanObservation::TemperatureMillidegrees(70_000))
            .unwrap();
        let before = engine.state();
        assert_eq!(
            engine.evaluate(1_999, FanObservation::SensorUnavailable),
            Err(FanPolicyError::ClockMovedBackwards)
        );
        assert_eq!(engine.state(), before);
    }
}
