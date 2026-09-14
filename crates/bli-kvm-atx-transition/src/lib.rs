//! Persisted ATX transition state machine.
//!
//! The engine combines the persisted `bli-kvm-atx-policy` journal with the
//! fixed-route `bli-kvm-kvmd-atx-client`. A permit persists a Pending record,
//! the fixed waiting route is called once, and the completion is persisted
//! before any response. Duplicates and conflicts return the persisted outcome
//! without another backend call. Startup reconciliation is an explicit
//! operation that must be backed by read-only LED/state evidence.
//!
//! The ATX capability stays unavailable: shipped builds can only construct
//! [`bli_kvm_atx_policy::AtxEligibility::disabled`].

use bli_kvm_atx_policy::{
    AtxCompletion, AtxDecision, AtxEligibility, AtxOperation, AtxPolicyEngine, AtxPolicyError,
    AtxRejection,
};
use bli_kvm_core::AtxControlAction;
#[cfg(feature = "kvmd-atx-profile")]
use bli_kvm_kvmd_atx_client::KvmdAtxClient;
#[cfg(feature = "kvmd-atx-profile")]
use bli_kvm_kvmd_client::KvmdCredentials;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AtxTransitionError {
    #[error("ATX policy journal failed")]
    Policy(#[from] AtxPolicyError),
    #[error("ATX reconciliation requires read-only power-LED evidence")]
    EvidenceRequired,
}

/// KVMD-backed ATX executor. Holds its own credential instance.
#[cfg(feature = "kvmd-atx-profile")]
pub struct KvmdAtxExecutor {
    client: KvmdAtxClient,
    credentials: KvmdCredentials,
}

#[cfg(feature = "kvmd-atx-profile")]
impl KvmdAtxExecutor {
    pub fn new(client: KvmdAtxClient, credentials: KvmdCredentials) -> Self {
        Self {
            client,
            credentials,
        }
    }

    async fn execute(&self, action: AtxControlAction) -> bool {
        self.client.execute(action, &self.credentials).await.is_ok()
    }
}

/// Reviewed actuator capable of one bounded ATX pulse. Implemented by the
/// GPIO executor; kept as a trait so the transition engine needs no GPIO
/// dependency.
pub trait AtxPulseRunner: Send {
    fn execute(&mut self, action: AtxControlAction) -> Result<(), String>;
}

/// Backend that actually pulses the ATX lines.
///
/// The transition engine is backend-agnostic so the KVMD executor used during
/// migration can later be replaced by the Rust board executor without touching
/// the policy or the broker wiring.
pub enum AtxExecutor {
    #[cfg(feature = "kvmd-atx-profile")]
    Kvmd(KvmdAtxExecutor),
    #[cfg(feature = "gpio-atx-profile")]
    Gpio(std::sync::Mutex<Box<dyn AtxPulseRunner>>),
    #[cfg(test)]
    Test(bool),
}

impl AtxExecutor {
    #[cfg(not(any(feature = "kvmd-atx-profile", feature = "gpio-atx-profile")))]
    async fn execute(&self, _action: AtxControlAction) -> bool {
        false
    }

    #[cfg(any(feature = "kvmd-atx-profile", feature = "gpio-atx-profile"))]
    async fn execute(&self, action: AtxControlAction) -> bool {
        match self {
            #[cfg(feature = "kvmd-atx-profile")]
            AtxExecutor::Kvmd(executor) => executor.execute(action).await,
            #[cfg(feature = "gpio-atx-profile")]
            AtxExecutor::Gpio(runner) => runner
                .lock()
                .map(|mut runner| runner.execute(action).is_ok())
                .unwrap_or(false),
            #[cfg(test)]
            AtxExecutor::Test(applied) => *applied,
        }
    }
}

/// Read-only power-LED observation used when reconciling a Pending operation.
///
/// The observation is mandatory evidence; it never infers the completion.
/// The operator still classifies the outcome explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerLedObservation {
    On,
    Off,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtxTransitionOutcome {
    Applied,
    ReplayedApplied,
    ReplayedRejected,
    BackendFailed,
    Rejected(AtxRejection),
}

pub struct AtxTransitionEngine {
    policy: AtxPolicyEngine,
    executor: AtxExecutor,
}

impl AtxTransitionEngine {
    #[cfg(feature = "kvmd-atx-profile")]
    pub fn open(
        journal_path: impl Into<std::path::PathBuf>,
        client: KvmdAtxClient,
        credentials: KvmdCredentials,
    ) -> Result<Self, AtxTransitionError> {
        Self::open_with_executor(
            journal_path,
            AtxExecutor::Kvmd(KvmdAtxExecutor::new(client, credentials)),
        )
    }

    /// Build an engine over any reviewed executor backend.
    pub fn open_with_executor(
        journal_path: impl Into<std::path::PathBuf>,
        executor: AtxExecutor,
    ) -> Result<Self, AtxTransitionError> {
        Ok(Self {
            policy: AtxPolicyEngine::open(journal_path)?,
            executor,
        })
    }

    pub fn pending_count(&self) -> usize {
        self.policy.pending_count()
    }

    pub fn pending_operation_ids(&self) -> Vec<[u8; 16]> {
        self.policy.pending_operation_ids()
    }

    pub async fn execute(
        &mut self,
        operation: AtxOperation,
        eligibility: AtxEligibility,
        now_ms: u64,
    ) -> Result<AtxTransitionOutcome, AtxTransitionError> {
        let decision = self.policy.prepare(operation, eligibility, now_ms)?;
        match decision {
            AtxDecision::Reject(rejection) => Ok(AtxTransitionOutcome::Rejected(rejection)),
            AtxDecision::ReplayApplied => Ok(AtxTransitionOutcome::ReplayedApplied),
            AtxDecision::ReplayRejected => Ok(AtxTransitionOutcome::ReplayedRejected),
            AtxDecision::Permit => {
                let operation_id = operation.operation_id();
                let applied = self.executor.execute(operation.action()).await;
                let completion = if applied {
                    AtxCompletion::Applied
                } else {
                    AtxCompletion::Rejected
                };
                self.policy.complete(operation_id, completion, now_ms)?;
                if applied {
                    Ok(AtxTransitionOutcome::Applied)
                } else {
                    Ok(AtxTransitionOutcome::BackendFailed)
                }
            }
        }
    }

    pub fn resolve_pending(
        &mut self,
        operation_id: [u8; 16],
        completion: AtxCompletion,
        observation: PowerLedObservation,
        reconciled_at_ms: u64,
    ) -> Result<(), AtxTransitionError> {
        if observation == PowerLedObservation::Unknown {
            return Err(AtxTransitionError::EvidenceRequired);
        }
        self.policy
            .resolve_pending(operation_id, completion, reconciled_at_ms)
            .map_err(AtxTransitionError::Policy)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use bli_kvm_core::AtxControlAction;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
    };

    use super::*;

    async fn serve(socket: &Path, statuses: &[&str]) -> tokio::task::JoinHandle<Vec<String>> {
        let listener = UnixListener::bind(socket).unwrap();
        let statuses = statuses
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();
        tokio::spawn(async move {
            let mut requests = Vec::new();
            for status in statuses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0_u8; 1024];
                    let read = stream.read(&mut chunk).await.unwrap();
                    request.extend_from_slice(&chunk[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let body = if status.starts_with("200") {
                    r#"{"ok":true}"#
                } else {
                    r#"{"ok":false}"#
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                requests.push(String::from_utf8(request).unwrap());
            }
            requests
        })
    }

    fn credentials() -> KvmdCredentials {
        KvmdCredentials::new("test-user", "not-a-real-password").unwrap()
    }

    fn engine(socket: &Path, journal: &Path) -> AtxTransitionEngine {
        AtxTransitionEngine::open(journal, KvmdAtxClient::new(socket).unwrap(), credentials())
            .unwrap()
    }

    fn reviewed() -> AtxEligibility {
        AtxEligibility::reviewed_for_confirmation(9, [0x44; 32], 1_000, 1_500, true, true).unwrap()
    }

    fn operation(seed: u8, action: AtxControlAction) -> AtxOperation {
        AtxOperation::new([seed; 16], u64::from(seed), [seed; 32], 1_000, action).unwrap()
    }

    #[tokio::test]
    async fn engine_executes_through_any_reviewed_backend() {
        let directory = tempfile::tempdir().unwrap();
        let mut engine = AtxTransitionEngine::open_with_executor(
            directory.path().join("journal.json"),
            AtxExecutor::Test(true),
        )
        .unwrap();
        assert_eq!(
            engine
                .execute(
                    operation(1, AtxControlAction::PowerShort),
                    reviewed(),
                    1_500
                )
                .await
                .unwrap(),
            AtxTransitionOutcome::Applied
        );
        assert_eq!(
            engine
                .execute(
                    operation(1, AtxControlAction::PowerShort),
                    reviewed(),
                    1_500
                )
                .await
                .unwrap(),
            AtxTransitionOutcome::ReplayedApplied
        );

        let mut failing = AtxTransitionEngine::open_with_executor(
            directory.path().join("failing.json"),
            AtxExecutor::Test(false),
        )
        .unwrap();
        assert_eq!(
            failing
                .execute(operation(2, AtxControlAction::Reset), reviewed(), 1_800)
                .await
                .unwrap(),
            AtxTransitionOutcome::BackendFailed
        );
    }

    #[tokio::test]
    async fn permit_calls_the_fixed_route_once_and_duplicates_replay() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let server = serve(&socket, &["200 OK"]).await;
        let mut engine = engine(&socket, &directory.path().join("journal.json"));
        let request = operation(1, AtxControlAction::PowerShort);

        assert_eq!(
            engine.execute(request, reviewed(), 1_500).await.unwrap(),
            AtxTransitionOutcome::Applied
        );
        assert_eq!(
            engine.execute(request, reviewed(), 1_500).await.unwrap(),
            AtxTransitionOutcome::ReplayedApplied
        );
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("POST /atx/click?button=power&wait=1 HTTP/1.1\r\n"));
    }

    #[tokio::test]
    async fn conflicting_operation_id_is_rejected_without_a_backend_call() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let server = serve(&socket, &["200 OK"]).await;
        let mut engine = engine(&socket, &directory.path().join("journal.json"));

        assert_eq!(
            engine
                .execute(
                    operation(1, AtxControlAction::PowerShort),
                    reviewed(),
                    1_500
                )
                .await
                .unwrap(),
            AtxTransitionOutcome::Applied
        );
        assert_eq!(
            engine
                .execute(operation(1, AtxControlAction::Reset), reviewed(), 1_500)
                .await
                .unwrap(),
            AtxTransitionOutcome::Rejected(AtxRejection::OperationIdConflict)
        );
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn backend_failure_is_persisted_and_duplicates_do_not_retry() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let server = serve(&socket, &["500 Error"]).await;
        let mut engine = engine(&socket, &directory.path().join("journal.json"));
        let request = operation(1, AtxControlAction::PowerLong);

        assert_eq!(
            engine.execute(request, reviewed(), 1_500).await.unwrap(),
            AtxTransitionOutcome::BackendFailed
        );
        assert_eq!(
            engine.execute(request, reviewed(), 1_600).await.unwrap(),
            AtxTransitionOutcome::ReplayedRejected
        );
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("/atx/click?button=power_long&wait=1"));
    }

    #[tokio::test]
    async fn a_pending_operation_blocks_a_new_action_until_reconciled() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("missing-kvmd.sock");
        let journal = directory.path().join("journal.json");

        {
            let mut policy = AtxPolicyEngine::open(&journal).unwrap();
            assert_eq!(
                policy
                    .prepare(operation(1, AtxControlAction::Reset), reviewed(), 1_500)
                    .unwrap(),
                AtxDecision::Permit
            );
        }

        let mut engine = AtxTransitionEngine::open(
            &journal,
            KvmdAtxClient::new(&socket).unwrap(),
            credentials(),
        )
        .unwrap();
        assert_eq!(engine.pending_count(), 1);
        assert_eq!(engine.pending_operation_ids(), vec![[1; 16]]);
        assert_eq!(
            engine
                .execute(
                    operation(2, AtxControlAction::PowerShort),
                    reviewed(),
                    1_600
                )
                .await
                .unwrap(),
            AtxTransitionOutcome::Rejected(AtxRejection::IndeterminatePending)
        );

        engine
            .resolve_pending(
                [1; 16],
                AtxCompletion::Applied,
                PowerLedObservation::On,
                1_700,
            )
            .unwrap();
        assert_eq!(engine.pending_count(), 0);
        assert_eq!(
            engine
                .execute(operation(1, AtxControlAction::Reset), reviewed(), 1_800)
                .await
                .unwrap(),
            AtxTransitionOutcome::ReplayedApplied
        );
        assert!(!socket.exists());
    }

    #[tokio::test]
    async fn disabled_eligibility_never_reaches_the_backend() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("missing-kvmd.sock");
        let mut engine = engine(&socket, &directory.path().join("journal.json"));

        assert_eq!(
            engine
                .execute(
                    operation(1, AtxControlAction::PowerShort),
                    AtxEligibility::disabled(),
                    1_500
                )
                .await
                .unwrap(),
            AtxTransitionOutcome::Rejected(AtxRejection::CapabilityDisabled)
        );
        assert_eq!(engine.pending_count(), 0);
        assert!(!socket.exists());
    }

    #[tokio::test]
    async fn reconciliation_requires_read_only_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let journal = directory.path().join("journal.json");
        {
            let mut policy = AtxPolicyEngine::open(&journal).unwrap();
            assert_eq!(
                policy
                    .prepare(operation(1, AtxControlAction::Reset), reviewed(), 1_500)
                    .unwrap(),
                AtxDecision::Permit
            );
        }
        let socket = directory.path().join("missing-kvmd.sock");
        let mut engine = AtxTransitionEngine::open(
            &journal,
            KvmdAtxClient::new(&socket).unwrap(),
            credentials(),
        )
        .unwrap();
        assert!(matches!(
            engine.resolve_pending(
                [1; 16],
                AtxCompletion::Applied,
                PowerLedObservation::Unknown,
                1_700
            ),
            Err(AtxTransitionError::EvidenceRequired)
        ));
        assert_eq!(engine.pending_count(), 1);
        engine
            .resolve_pending(
                [1; 16],
                AtxCompletion::Applied,
                PowerLedObservation::Off,
                1_700,
            )
            .unwrap();
        assert_eq!(engine.pending_count(), 0);
        assert!(!socket.exists());
    }

    #[tokio::test]
    async fn busy_backend_response_is_persisted_without_retry() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let server = serve(&socket, &["409 Conflict"]).await;
        let mut engine = engine(&socket, &directory.path().join("journal.json"));
        let request = operation(1, AtxControlAction::PowerShort);

        assert_eq!(
            engine.execute(request, reviewed(), 1_500).await.unwrap(),
            AtxTransitionOutcome::BackendFailed
        );
        assert_eq!(
            engine.execute(request, reviewed(), 1_600).await.unwrap(),
            AtxTransitionOutcome::ReplayedRejected
        );
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("/atx/click?button=power&wait=1"));
    }

    #[cfg(feature = "gpio-atx-profile")]
    #[tokio::test]
    async fn engine_executes_through_the_gpio_backend() {
        struct RecordingRunner(std::sync::Arc<std::sync::Mutex<Vec<AtxControlAction>>>);
        impl AtxPulseRunner for RecordingRunner {
            fn execute(&mut self, action: AtxControlAction) -> Result<(), String> {
                self.0.lock().unwrap().push(action);
                Ok(())
            }
        }

        let directory = tempfile::tempdir().unwrap();
        let recorded = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let executor = AtxExecutor::Gpio(std::sync::Mutex::new(Box::new(RecordingRunner(
            recorded.clone(),
        ))));
        let mut engine = AtxTransitionEngine::open_with_executor(
            directory.path().join("journal.json"),
            executor,
        )
        .unwrap();
        assert_eq!(
            engine
                .execute(
                    operation(1, AtxControlAction::PowerShort),
                    reviewed(),
                    1_500
                )
                .await
                .unwrap(),
            AtxTransitionOutcome::Applied
        );
        assert_eq!(
            *recorded.lock().unwrap(),
            vec![AtxControlAction::PowerShort]
        );
    }
}
