//! Mockable fan command supervisor with no concrete hardware implementation.

use std::error::Error;

use bli_kvm_fan_policy::{
    FAILSAFE_DUTY_PERCENT, FanCommand, FanConfigError, FanDecision, FanDutyPercent, FanObservation,
    FanPolicyConfig, FanPolicyEngine, FanPolicyError, FanPolicyState,
};
use thiserror::Error;

pub trait FanActuator {
    type Error: Error + Send + Sync + 'static;

    fn apply(&mut self, command: FanCommand) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanSupervisorStatus {
    Uninitialized,
    Active,
    FailsafeLatched,
    ActuationUncertain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanSupervisorAction {
    StartupFailsafeApplied,
    CommandApplied,
    NoChange,
    FailsafeLatched,
    FailsafeRetryApplied,
    ShutdownFailsafeApplied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanSupervisorOutcome {
    pub action: FanSupervisorAction,
    pub status: FanSupervisorStatus,
    pub applied_command: Option<FanCommand>,
    pub policy_decision: Option<FanDecision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanSupervisorState {
    pub status: FanSupervisorStatus,
    pub applied_command: Option<FanCommand>,
    pub policy: FanPolicyState,
}

#[derive(Debug, Error)]
pub enum FanSupervisorError<E>
where
    E: Error + Send + Sync + 'static,
{
    #[error("fan supervisor is not initialized")]
    NotInitialized,
    #[error("fan supervisor is already initialized")]
    AlreadyInitialized,
    #[error("fan actuation state is uncertain; only failsafe retry is allowed")]
    ActuationUncertain,
    #[error("fan failsafe retry is not required")]
    RetryNotRequired,
    #[error("fan policy evaluation failed")]
    Policy(#[source] FanPolicyError),
    #[error("startup failsafe command was not confirmed")]
    StartupFailsafe(#[source] E),
    #[error("fan command failed; maximum-cooling fallback was confirmed and latched")]
    CommandFailedFailsafeApplied(#[source] E),
    #[error("fan command and maximum-cooling fallback both failed")]
    CommandAndFailsafeFailed { command_error: E, failsafe_error: E },
    #[error("maximum-cooling retry failed")]
    FailsafeRetryFailed(#[source] E),
    #[error("controlled shutdown maximum-cooling command failed")]
    ShutdownFailsafeFailed(#[source] E),
}

pub struct FanSupervisor<A>
where
    A: FanActuator,
{
    policy: FanPolicyEngine,
    actuator: A,
    status: FanSupervisorStatus,
    applied_command: Option<FanCommand>,
}

impl<A> FanSupervisor<A>
where
    A: FanActuator,
{
    pub fn new(
        config: FanPolicyConfig,
        started_at_ms: u64,
        actuator: A,
    ) -> Result<Self, FanConfigError> {
        Ok(Self {
            policy: FanPolicyEngine::new(config, started_at_ms)?,
            actuator,
            status: FanSupervisorStatus::Uninitialized,
            applied_command: None,
        })
    }

    pub fn state(&self) -> FanSupervisorState {
        FanSupervisorState {
            status: self.status,
            applied_command: self.applied_command,
            policy: self.policy.state(),
        }
    }

    pub fn actuator(&self) -> &A {
        &self.actuator
    }

    pub fn initialize(&mut self) -> Result<FanSupervisorOutcome, FanSupervisorError<A::Error>> {
        match self.status {
            FanSupervisorStatus::Uninitialized => {}
            FanSupervisorStatus::ActuationUncertain => {
                return Err(FanSupervisorError::ActuationUncertain);
            }
            FanSupervisorStatus::Active | FanSupervisorStatus::FailsafeLatched => {
                return Err(FanSupervisorError::AlreadyInitialized);
            }
        }
        let failsafe = failsafe_command();
        if let Err(error) = self.actuator.apply(failsafe) {
            self.status = FanSupervisorStatus::ActuationUncertain;
            self.applied_command = None;
            return Err(FanSupervisorError::StartupFailsafe(error));
        }
        self.status = FanSupervisorStatus::Active;
        self.applied_command = Some(failsafe);
        Ok(self.outcome(FanSupervisorAction::StartupFailsafeApplied, None))
    }

    pub fn evaluate(
        &mut self,
        now_ms: u64,
        observation: FanObservation,
    ) -> Result<FanSupervisorOutcome, FanSupervisorError<A::Error>> {
        match self.status {
            FanSupervisorStatus::Uninitialized => {
                return Err(FanSupervisorError::NotInitialized);
            }
            FanSupervisorStatus::ActuationUncertain => {
                return Err(FanSupervisorError::ActuationUncertain);
            }
            FanSupervisorStatus::FailsafeLatched => {
                return Ok(self.outcome(FanSupervisorAction::FailsafeLatched, None));
            }
            FanSupervisorStatus::Active => {}
        }

        let decision = self
            .policy
            .evaluate(now_ms, observation)
            .map_err(FanSupervisorError::Policy)?;
        if self.applied_command == Some(decision.command) {
            return Ok(self.outcome(FanSupervisorAction::NoChange, Some(decision)));
        }
        match self.actuator.apply(decision.command) {
            Ok(()) => {
                self.applied_command = Some(decision.command);
                Ok(self.outcome(FanSupervisorAction::CommandApplied, Some(decision)))
            }
            Err(command_error) => {
                let failsafe = failsafe_command();
                match self.actuator.apply(failsafe) {
                    Ok(()) => {
                        self.applied_command = Some(failsafe);
                        self.status = FanSupervisorStatus::FailsafeLatched;
                        Err(FanSupervisorError::CommandFailedFailsafeApplied(
                            command_error,
                        ))
                    }
                    Err(failsafe_error) => {
                        self.status = FanSupervisorStatus::ActuationUncertain;
                        Err(FanSupervisorError::CommandAndFailsafeFailed {
                            command_error,
                            failsafe_error,
                        })
                    }
                }
            }
        }
    }

    pub fn retry_failsafe(&mut self) -> Result<FanSupervisorOutcome, FanSupervisorError<A::Error>> {
        if self.status != FanSupervisorStatus::ActuationUncertain {
            return Err(FanSupervisorError::RetryNotRequired);
        }
        let failsafe = failsafe_command();
        match self.actuator.apply(failsafe) {
            Ok(()) => {
                self.applied_command = Some(failsafe);
                self.status = FanSupervisorStatus::FailsafeLatched;
                Ok(self.outcome(FanSupervisorAction::FailsafeRetryApplied, None))
            }
            Err(error) => Err(FanSupervisorError::FailsafeRetryFailed(error)),
        }
    }

    pub fn prepare_shutdown(
        &mut self,
    ) -> Result<FanSupervisorOutcome, FanSupervisorError<A::Error>> {
        let failsafe = failsafe_command();
        match self.actuator.apply(failsafe) {
            Ok(()) => {
                self.applied_command = Some(failsafe);
                self.status = FanSupervisorStatus::FailsafeLatched;
                Ok(self.outcome(FanSupervisorAction::ShutdownFailsafeApplied, None))
            }
            Err(error) => {
                self.status = FanSupervisorStatus::ActuationUncertain;
                Err(FanSupervisorError::ShutdownFailsafeFailed(error))
            }
        }
    }

    pub fn into_actuator(self) -> A {
        self.actuator
    }

    fn outcome(
        &self,
        action: FanSupervisorAction,
        policy_decision: Option<FanDecision>,
    ) -> FanSupervisorOutcome {
        FanSupervisorOutcome {
            action,
            status: self.status,
            applied_command: self.applied_command,
            policy_decision,
        }
    }
}

fn failsafe_command() -> FanCommand {
    debug_assert_eq!(FanDutyPercent::MAXIMUM.get(), FAILSAFE_DUTY_PERCENT);
    FanCommand::DutyPercent(FanDutyPercent::MAXIMUM)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use bli_kvm_fan_policy::{FanCurvePoint, FanMode, FanReason};

    use super::*;

    #[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
    #[error("mock fan actuator failure {0}")]
    struct MockError(u8);

    #[derive(Debug, Default)]
    struct MockActuator {
        attempts: Vec<FanCommand>,
        results: VecDeque<Result<(), MockError>>,
    }

    impl MockActuator {
        fn with_results(results: impl IntoIterator<Item = Result<(), MockError>>) -> Self {
            Self {
                attempts: Vec::new(),
                results: results.into_iter().collect(),
            }
        }
    }

    impl FanActuator for MockActuator {
        type Error = MockError;

        fn apply(&mut self, command: FanCommand) -> Result<(), Self::Error> {
            self.attempts.push(command);
            self.results.pop_front().unwrap_or(Ok(()))
        }
    }

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
                    at_or_above_millidegrees: 50_000,
                    duty_percent: 70,
                },
                FanCurvePoint {
                    at_or_above_millidegrees: 60_000,
                    duty_percent: 100,
                },
            ],
        }
    }

    #[test]
    fn startup_failsafe_must_be_confirmed_before_samples() {
        let mut supervisor = FanSupervisor::new(config(), 0, MockActuator::default()).unwrap();
        assert!(matches!(
            supervisor.evaluate(0, FanObservation::TemperatureMillidegrees(50_000)),
            Err(FanSupervisorError::NotInitialized)
        ));
        assert!(supervisor.actuator().attempts.is_empty());

        let initialized = supervisor.initialize().unwrap();
        assert_eq!(
            initialized.action,
            FanSupervisorAction::StartupFailsafeApplied
        );
        assert_eq!(initialized.applied_command, Some(duty(100)));
        assert_eq!(supervisor.actuator().attempts, vec![duty(100)]);
        assert!(matches!(
            supervisor.initialize(),
            Err(FanSupervisorError::AlreadyInitialized)
        ));
    }

    #[test]
    fn unchanged_commands_are_not_reapplied() {
        let mut supervisor = FanSupervisor::new(config(), 0, MockActuator::default()).unwrap();
        let _ = supervisor.initialize().unwrap();
        let unchanged = supervisor
            .evaluate(0, FanObservation::TemperatureMillidegrees(70_000))
            .unwrap();
        assert_eq!(unchanged.action, FanSupervisorAction::NoChange);
        assert_eq!(supervisor.actuator().attempts, vec![duty(100)]);

        let off = supervisor
            .evaluate(30_000, FanObservation::TemperatureMillidegrees(50_000))
            .unwrap();
        assert_eq!(off.action, FanSupervisorAction::CommandApplied);
        assert_eq!(off.applied_command, Some(FanCommand::Off));
        assert_eq!(
            supervisor.actuator().attempts,
            vec![duty(100), FanCommand::Off]
        );
    }

    #[test]
    fn command_failure_applies_and_latches_maximum_cooling() {
        let actuator = MockActuator::with_results([Ok(()), Err(MockError(1)), Ok(())]);
        let mut supervisor = FanSupervisor::new(config(), 0, actuator).unwrap();
        let _ = supervisor.initialize().unwrap();
        assert!(matches!(
            supervisor.evaluate(30_000, FanObservation::TemperatureMillidegrees(50_000)),
            Err(FanSupervisorError::CommandFailedFailsafeApplied(MockError(
                1
            )))
        ));
        assert_eq!(
            supervisor.state().status,
            FanSupervisorStatus::FailsafeLatched
        );
        assert_eq!(supervisor.state().applied_command, Some(duty(100)));
        assert_eq!(
            supervisor.actuator().attempts,
            vec![duty(100), FanCommand::Off, duty(100)]
        );

        let latched = supervisor
            .evaluate(60_000, FanObservation::TemperatureMillidegrees(50_000))
            .unwrap();
        assert_eq!(latched.action, FanSupervisorAction::FailsafeLatched);
        assert_eq!(supervisor.actuator().attempts.len(), 3);
    }

    #[test]
    fn double_failure_blocks_samples_until_explicit_failsafe_retry() {
        let actuator = MockActuator::with_results([
            Ok(()),
            Err(MockError(1)),
            Err(MockError(2)),
            Err(MockError(3)),
            Ok(()),
        ]);
        let mut supervisor = FanSupervisor::new(config(), 0, actuator).unwrap();
        let _ = supervisor.initialize().unwrap();
        assert!(matches!(
            supervisor.evaluate(30_000, FanObservation::TemperatureMillidegrees(50_000)),
            Err(FanSupervisorError::CommandAndFailsafeFailed {
                command_error: MockError(1),
                failsafe_error: MockError(2),
            })
        ));
        assert_eq!(
            supervisor.state().status,
            FanSupervisorStatus::ActuationUncertain
        );
        assert!(matches!(
            supervisor.evaluate(31_000, FanObservation::SensorUnavailable),
            Err(FanSupervisorError::ActuationUncertain)
        ));
        assert!(matches!(
            supervisor.retry_failsafe(),
            Err(FanSupervisorError::FailsafeRetryFailed(MockError(3)))
        ));
        let recovered = supervisor.retry_failsafe().unwrap();
        assert_eq!(recovered.action, FanSupervisorAction::FailsafeRetryApplied);
        assert_eq!(recovered.status, FanSupervisorStatus::FailsafeLatched);
        assert_eq!(recovered.applied_command, Some(duty(100)));
    }

    #[test]
    fn startup_failure_never_accepts_a_sample_and_retry_stays_latched() {
        let actuator = MockActuator::with_results([Err(MockError(1)), Ok(())]);
        let mut supervisor = FanSupervisor::new(config(), 0, actuator).unwrap();
        assert!(matches!(
            supervisor.initialize(),
            Err(FanSupervisorError::StartupFailsafe(MockError(1)))
        ));
        assert_eq!(
            supervisor.state().status,
            FanSupervisorStatus::ActuationUncertain
        );
        assert!(matches!(
            supervisor.evaluate(1, FanObservation::TemperatureMillidegrees(50_000)),
            Err(FanSupervisorError::ActuationUncertain)
        ));
        let retry = supervisor.retry_failsafe().unwrap();
        assert_eq!(retry.status, FanSupervisorStatus::FailsafeLatched);
        assert_eq!(supervisor.actuator().attempts, vec![duty(100), duty(100)]);
    }

    #[test]
    fn policy_errors_do_not_reach_the_actuator() {
        let mut supervisor = FanSupervisor::new(config(), 1_000, MockActuator::default()).unwrap();
        let _ = supervisor.initialize().unwrap();
        let _ = supervisor
            .evaluate(2_000, FanObservation::TemperatureMillidegrees(70_000))
            .unwrap();
        let attempts = supervisor.actuator().attempts.len();
        assert!(matches!(
            supervisor.evaluate(1_999, FanObservation::SensorUnavailable),
            Err(FanSupervisorError::Policy(
                FanPolicyError::ClockMovedBackwards
            ))
        ));
        assert_eq!(supervisor.actuator().attempts.len(), attempts);
        assert_eq!(supervisor.state().policy.mode, FanMode::Running);
        assert_eq!(supervisor.state().policy.reason, FanReason::SensorRecovered);
    }

    #[test]
    fn a_new_supervisor_always_starts_with_maximum_cooling_again() {
        let mut first = FanSupervisor::new(config(), 0, MockActuator::default()).unwrap();
        let _ = first.initialize().unwrap();
        let _ = first
            .evaluate(30_000, FanObservation::TemperatureMillidegrees(50_000))
            .unwrap();
        assert_eq!(first.state().applied_command, Some(FanCommand::Off));

        let mut restarted = FanSupervisor::new(config(), 30_001, MockActuator::default()).unwrap();
        let _ = restarted.initialize().unwrap();
        assert_eq!(restarted.actuator().attempts, vec![duty(100)]);
        assert_eq!(restarted.state().policy.mode, FanMode::Failsafe);
    }

    #[test]
    fn controlled_shutdown_reconfirms_maximum_cooling_and_latches() {
        let mut supervisor = FanSupervisor::new(config(), 0, MockActuator::default()).unwrap();
        let _ = supervisor.initialize().unwrap();
        let _ = supervisor
            .evaluate(30_000, FanObservation::TemperatureMillidegrees(50_000))
            .unwrap();
        assert_eq!(supervisor.state().applied_command, Some(FanCommand::Off));

        let shutdown = supervisor.prepare_shutdown().unwrap();
        assert_eq!(
            shutdown.action,
            FanSupervisorAction::ShutdownFailsafeApplied
        );
        assert_eq!(shutdown.status, FanSupervisorStatus::FailsafeLatched);
        assert_eq!(shutdown.applied_command, Some(duty(100)));
        assert_eq!(
            supervisor.actuator().attempts,
            vec![duty(100), FanCommand::Off, duty(100)]
        );
    }

    #[test]
    fn controlled_shutdown_failure_is_actuation_uncertain() {
        let actuator = MockActuator::with_results([Ok(()), Err(MockError(1))]);
        let mut supervisor = FanSupervisor::new(config(), 0, actuator).unwrap();
        let _ = supervisor.initialize().unwrap();
        assert!(matches!(
            supervisor.prepare_shutdown(),
            Err(FanSupervisorError::ShutdownFailsafeFailed(MockError(1)))
        ));
        assert_eq!(
            supervisor.state().status,
            FanSupervisorStatus::ActuationUncertain
        );
    }
}
