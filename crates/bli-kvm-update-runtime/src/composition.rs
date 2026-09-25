//! Closed native application quiescence order. The manager must persist
//! Prepared intent and hold its transition locks before invoking this layer.

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplicationUnit {
    BoardServer,
    CurrentServer,
    CandidateServer,
    BootstrapServer,
    BoardDaemon,
    VideoProxy,
    Owner,
}

impl ApplicationUnit {
    pub const fn name(self) -> &'static str {
        match self {
            Self::BoardServer => "hardened-blikvm-board-server.service",
            Self::CurrentServer => "hardened-blikvm-server-current.service",
            Self::CandidateServer => "hardened-blikvm-server-candidate.service",
            Self::BootstrapServer => "hardened-blikvm-server.service",
            Self::BoardDaemon => "hardened-blikvm-boardd.service",
            Self::VideoProxy => "hardened-blikvm-board-video-proxy.service",
            Self::Owner => "hardened-blikvm-board-owner.service",
        }
    }
}

const PRODUCERS: &[ApplicationUnit] = &[
    ApplicationUnit::BoardServer,
    ApplicationUnit::CurrentServer,
    ApplicationUnit::CandidateServer,
    ApplicationUnit::BootstrapServer,
    ApplicationUnit::BoardDaemon,
];
const BACKENDS: &[ApplicationUnit] = &[ApplicationUnit::VideoProxy, ApplicationUnit::Owner];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopObservation {
    /// No main/control process or queued job, with a successful normal stop.
    /// A never-started, loaded inactive unit may also satisfy this condition.
    Clean,
    RunningOrQueued,
    /// Process-free and terminal, but the last run failed (a crash or a failed
    /// start). Counts as stopped, so recovery is not blocked by the failure it
    /// is meant to repair; the incoming units reinitialize their devices.
    StoppedAfterFailure,
    /// Failed while a main/control process, job or control group remains.
    Failed,
    Unknown,
}

impl StopObservation {
    /// No process, job or control group remains, whether or not the last
    /// run exited cleanly.
    pub fn is_stopped(self) -> bool {
        matches!(self, Self::Clean | Self::StoppedAfterFailure)
    }
}

#[derive(Debug, Error)]
pub enum CompositionError {
    #[error("verified slot does not contain the complete native composition")]
    IncompletePayload,
    #[error("native startup does not match the durable selected composition")]
    SelectionMismatch,
    #[error("application unit has an incompatible native startup contract: {0:?}")]
    IncompatibleStartup(ApplicationUnit),
    #[error("served web asset does not match the verified slot manifest")]
    WebAssetMismatch,
    #[error("application composition deadline expired or is invalid")]
    Deadline,
    #[error("application service operation failed")]
    Controller,
    #[error("application service client {operation} failed: {source}")]
    ClientIo {
        operation: &'static str,
        source: std::io::Error,
    },
    #[error("application service client failed: exit code {code:?}, signal {signal:?}")]
    ClientExit {
        code: Option<i32>,
        signal: Option<i32>,
    },
    #[error("application service client output exceeded its limit")]
    ClientOutputLimit,
    #[error("application unit did not stop cleanly: {0:?}")]
    NotStopped(ApplicationUnit),
}

const NATIVE_FILES: [(&str, u32); 5] = [
    ("bin/bli-kvm-server", 0o755),
    ("libexec/bli-kvm-control-broker", 0o755),
    ("libexec/bli-kvm-video-proxy", 0o755),
    ("libexec/bli-kvm-boardd", 0o755),
    ("share/web/index.html", 0o644),
];

/// Additional native inventory gate over an already verified slot manifest.
/// This does not replace signature/hash validation or runtime identity/readiness.
pub(crate) fn validate_native_payload<'a>(
    files: impl IntoIterator<Item = (&'a str, u32, u64)>,
) -> Result<(), CompositionError> {
    let mut seen = [false; NATIVE_FILES.len()];
    for (path, mode, size) in files {
        if let Some(index) = NATIVE_FILES
            .iter()
            .position(|(required, _)| *required == path)
        {
            if seen[index] || mode != NATIVE_FILES[index].1 || size == 0 {
                return Err(CompositionError::IncompletePayload);
            }
            seen[index] = true;
        }
    }
    if seen.into_iter().all(|present| present) {
        Ok(())
    } else {
        Err(CompositionError::IncompletePayload)
    }
}

/// Implementations must use a shared finite deadline for the whole quiescence,
/// bound every operation by its remaining time, and use these exact unit names,
/// and observe process/job/exit state rather than treating stop dispatch as
/// completion. No unit name or executable command is supplied by the payload.
pub trait ServiceController {
    fn stop(&mut self, unit: ApplicationUnit) -> Result<(), CompositionError>;
    fn observe_stop(&mut self, unit: ApplicationUnit) -> Result<StopObservation, CompositionError>;
}

/// Close producers before touching device owners. Dispatch all stops in a
/// phase, then require every member clean; this does not restart any unit or
/// change the selected slot. On failure the caller must retain recovery intent.
pub fn quiesce(controller: &mut impl ServiceController) -> Result<(), CompositionError> {
    for phase in [PRODUCERS, BACKENDS] {
        for &unit in phase {
            controller.stop(unit)?;
        }
        for &unit in phase {
            if !controller.observe_stop(unit)?.is_stopped() {
                return Err(CompositionError::NotStopped(unit));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_inventory_requires_each_nonempty_correctly_moded_component() {
        let complete: Vec<_> = NATIVE_FILES
            .iter()
            .map(|&(path, mode)| (path, mode, 1))
            .collect();
        validate_native_payload(complete.iter().copied()).unwrap();
        for index in 0..complete.len() {
            let mut missing = complete.clone();
            missing.remove(index);
            assert!(validate_native_payload(missing).is_err());
            let mut empty = complete.clone();
            empty[index].2 = 0;
            assert!(validate_native_payload(empty).is_err());
            let mut mode = complete.clone();
            mode[index].1 = 0o777;
            assert!(validate_native_payload(mode).is_err());
        }
        let mut duplicate = complete.clone();
        duplicate.push(complete[0]);
        assert!(validate_native_payload(duplicate).is_err());
    }

    #[derive(Default)]
    struct Controller {
        stopped: Vec<ApplicationUnit>,
        observed: Vec<ApplicationUnit>,
        fault: Option<(ApplicationUnit, StopObservation)>,
        dispatch_failure: Option<ApplicationUnit>,
    }

    impl ServiceController for Controller {
        fn stop(&mut self, unit: ApplicationUnit) -> Result<(), CompositionError> {
            if BACKENDS.contains(&unit) {
                assert_eq!(&self.observed[..PRODUCERS.len()], PRODUCERS);
            }
            self.stopped.push(unit);
            if self.dispatch_failure == Some(unit) {
                return Err(CompositionError::Controller);
            }
            Ok(())
        }

        fn observe_stop(
            &mut self,
            unit: ApplicationUnit,
        ) -> Result<StopObservation, CompositionError> {
            self.observed.push(unit);
            Ok(self
                .fault
                .filter(|(failed, _)| *failed == unit)
                .map_or(StopObservation::Clean, |(_, state)| state))
        }
    }

    #[test]
    fn owners_are_stopped_only_after_all_producers_are_clean() {
        let mut controller = Controller::default();
        quiesce(&mut controller).unwrap();
        assert_eq!(controller.stopped, [PRODUCERS, BACKENDS].concat());
        assert_eq!(controller.observed, controller.stopped);
    }

    #[test]
    fn a_unit_stopped_after_failure_does_not_block_quiescence() {
        for &unit in PRODUCERS.iter().chain(BACKENDS) {
            let mut controller = Controller {
                fault: Some((unit, StopObservation::StoppedAfterFailure)),
                ..Controller::default()
            };
            quiesce(&mut controller).unwrap();
            assert_eq!(controller.stopped, [PRODUCERS, BACKENDS].concat());
        }
    }

    #[test]
    fn each_unclean_unit_prevents_success_and_producer_failure_protects_owners() {
        for &unit in PRODUCERS.iter().chain(BACKENDS) {
            for state in [
                StopObservation::RunningOrQueued,
                StopObservation::Failed,
                StopObservation::Unknown,
            ] {
                let mut controller = Controller {
                    fault: Some((unit, state)),
                    ..Controller::default()
                };
                assert!(
                    matches!(quiesce(&mut controller), Err(CompositionError::NotStopped(failed)) if failed == unit)
                );
                if PRODUCERS.contains(&unit) {
                    assert!(
                        !controller
                            .stopped
                            .iter()
                            .any(|unit| BACKENDS.contains(unit))
                    );
                }
            }
        }
    }

    #[test]
    fn dispatch_failure_is_not_treated_as_an_observation() {
        let mut controller = Controller {
            dispatch_failure: Some(ApplicationUnit::BoardServer),
            ..Controller::default()
        };
        assert!(quiesce(&mut controller).is_err());
        assert!(controller.observed.is_empty());
        assert_eq!(controller.stopped, [ApplicationUnit::BoardServer]);
    }
}
