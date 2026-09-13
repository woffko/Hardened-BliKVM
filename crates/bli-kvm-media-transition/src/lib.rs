//! Persisted virtual-media transition state machine.
//!
//! The engine combines the private `bli-kvm-media-policy` journal with the
//! fixed-route `bli-kvm-kvmd-media-client` and the opaque-ID catalog. A permit
//! persists Pending, exactly one fixed KVMD route is called, and the completion
//! is persisted before the outcome is returned. Duplicates and conflicts return
//! the persisted result without another backend call.

use bli_kvm_kvmd_client::KvmdCredentials;
use bli_kvm_kvmd_media_client::KvmdMediaClient;
use bli_kvm_media_catalog::MediaCatalog;
use bli_kvm_media_policy::{
    MediaCompletion, MediaDecision, MediaEligibility, MediaObservedState, MediaOperation,
    MediaPolicyEngine, MediaPolicyError, MediaRejection,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MediaTransitionError {
    #[error("media policy journal failed")]
    Policy(#[from] MediaPolicyError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaTransitionOutcome {
    Applied,
    Noop,
    ReplayedApplied,
    ReplayedRejected,
    BackendFailed,
    Rejected(MediaRejection),
}

pub struct MediaTransitionEngine {
    policy: MediaPolicyEngine,
    client: KvmdMediaClient,
    catalog: MediaCatalog,
    credentials: KvmdCredentials,
}

impl MediaTransitionEngine {
    pub fn open(
        journal_path: impl Into<std::path::PathBuf>,
        client: KvmdMediaClient,
        catalog: MediaCatalog,
        credentials: KvmdCredentials,
    ) -> Result<Self, MediaTransitionError> {
        Ok(Self {
            policy: MediaPolicyEngine::open(journal_path)?,
            client,
            catalog,
            credentials,
        })
    }

    pub fn pending_count(&self) -> usize {
        self.policy.pending_count()
    }

    pub async fn execute(
        &mut self,
        operation: MediaOperation,
        eligibility: MediaEligibility,
        observed: MediaObservedState,
        now_ms: u64,
    ) -> Result<MediaTransitionOutcome, MediaTransitionError> {
        let decision = self
            .policy
            .prepare(operation, eligibility, observed, now_ms)?;
        match decision {
            MediaDecision::Reject(rejection) => Ok(MediaTransitionOutcome::Rejected(rejection)),
            MediaDecision::Noop => Ok(MediaTransitionOutcome::Noop),
            MediaDecision::ReplayApplied => Ok(MediaTransitionOutcome::ReplayedApplied),
            MediaDecision::ReplayRejected => Ok(MediaTransitionOutcome::ReplayedRejected),
            MediaDecision::Permit => {
                let operation_id = operation.operation_id();
                let command = operation.command();
                let applied = self
                    .client
                    .execute(&command, &self.catalog, &self.credentials)
                    .await
                    .is_ok();
                let completion = if applied {
                    MediaCompletion::Applied
                } else {
                    MediaCompletion::Rejected
                };
                self.policy.complete(operation_id, completion, now_ms)?;
                if applied {
                    Ok(MediaTransitionOutcome::Applied)
                } else {
                    Ok(MediaTransitionOutcome::BackendFailed)
                }
            }
        }
    }

    pub fn resolve_pending(
        &mut self,
        operation_id: [u8; 16],
        completion: MediaCompletion,
        reconciled_at_ms: u64,
    ) -> Result<(), MediaTransitionError> {
        self.policy
            .resolve_pending(operation_id, completion, reconciled_at_ms)
            .map_err(MediaTransitionError::Policy)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use bli_kvm_core::{VirtualMediaCommand, VirtualMediaControlMode};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
    };

    use super::*;

    const CATALOG: &str = r#"{
        "schemaVersion": 1,
        "entries": [{
            "imageId": 7,
            "imageName": "rescue.iso",
            "size": 1,
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "cdromAllowed": true,
            "removableAllowed": false,
            "writableAllowed": false
        }]
    }"#;

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
                requests.push(String::from_utf8_lossy(&request).into_owned());
            }
            requests
        })
    }

    fn catalog(directory: &Path) -> MediaCatalog {
        let path = directory.join("catalog.json");
        std::fs::write(&path, CATALOG).unwrap();
        MediaCatalog::open(&path).unwrap()
    }

    fn credentials() -> KvmdCredentials {
        KvmdCredentials::new("test-user", "not-a-real-password").unwrap()
    }

    fn engine(directory: &Path, socket: &Path) -> MediaTransitionEngine {
        MediaTransitionEngine::open(
            directory.join("journal.json"),
            KvmdMediaClient::new(socket).unwrap(),
            catalog(directory),
            credentials(),
        )
        .unwrap()
    }

    fn observed(busy: bool, connected: bool) -> MediaObservedState {
        MediaObservedState {
            enabled: true,
            online: true,
            busy,
            connected,
        }
    }

    fn operation(seed: u8, command: VirtualMediaCommand) -> MediaOperation {
        MediaOperation::new([seed; 16], 9, [0x44; 32], 1_000, command).unwrap()
    }

    #[tokio::test]
    async fn set_connected_is_applied_once_and_replays() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("media.sock");
        let server = serve(&socket, &["200 OK"]).await;
        let mut engine = engine(directory.path(), &socket);
        let request = operation(1, VirtualMediaCommand::SetConnected { connected: true });
        let eligibility = MediaEligibility::verified_for_test_profile();

        assert_eq!(
            engine
                .execute(request, eligibility, observed(false, false), 1_500)
                .await
                .unwrap(),
            MediaTransitionOutcome::Applied
        );
        assert_eq!(
            engine
                .execute(request, eligibility, observed(false, true), 1_600)
                .await
                .unwrap(),
            MediaTransitionOutcome::ReplayedApplied
        );
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("POST /msd/set_connected?connected=1 HTTP/1.1\r\n"));
    }

    #[tokio::test]
    async fn select_uses_only_the_catalog_route() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("media.sock");
        let server = serve(&socket, &["200 OK"]).await;
        let mut engine = engine(directory.path(), &socket);
        let request = operation(
            2,
            VirtualMediaCommand::Select {
                image_id: 7,
                mode: VirtualMediaControlMode::CdRom,
                writable: false,
            },
        );

        assert_eq!(
            engine
                .execute(
                    request,
                    MediaEligibility::verified_for_test_profile(),
                    observed(false, false),
                    1_500
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::Applied
        );
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]
                .starts_with("POST /msd/set_params?image=rescue.iso&cdrom=1&rw=0 HTTP/1.1\r\n")
        );
        assert!(!requests[0].contains("/dev/"));
    }

    #[tokio::test]
    async fn busy_and_must_disconnect_fail_without_backend_io() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("absent.sock");
        let mut engine = engine(directory.path(), &socket);
        let eligibility = MediaEligibility::verified_for_test_profile();

        assert_eq!(
            engine
                .execute(
                    operation(3, VirtualMediaCommand::SetConnected { connected: true }),
                    eligibility,
                    observed(true, false),
                    1_500
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::Rejected(MediaRejection::Busy)
        );
        assert_eq!(
            engine
                .execute(
                    operation(
                        4,
                        VirtualMediaCommand::Select {
                            image_id: 7,
                            mode: VirtualMediaControlMode::CdRom,
                            writable: false
                        }
                    ),
                    eligibility,
                    observed(false, true),
                    1_500
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::Rejected(MediaRejection::MustDisconnect)
        );
        assert_eq!(
            engine
                .execute(
                    operation(5, VirtualMediaCommand::SetConnected { connected: true }),
                    eligibility,
                    observed(false, true),
                    1_500
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::Noop
        );
        assert_eq!(engine.pending_count(), 0);
        assert!(!socket.exists());
    }

    #[tokio::test]
    async fn backend_failure_is_persisted_without_retry() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("media.sock");
        let server = serve(&socket, &["500 Error"]).await;
        let mut engine = engine(directory.path(), &socket);
        let request = operation(6, VirtualMediaCommand::SetConnected { connected: true });

        assert_eq!(
            engine
                .execute(
                    request,
                    MediaEligibility::verified_for_test_profile(),
                    observed(false, false),
                    1_500
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::BackendFailed
        );
        assert_eq!(
            engine
                .execute(
                    request,
                    MediaEligibility::verified_for_test_profile(),
                    observed(false, false),
                    1_600
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::ReplayedRejected
        );
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn pending_survives_restart_and_requires_reconciliation() {
        use bli_kvm_media_policy::{MediaDecision, MediaPolicyEngine};

        let directory = tempfile::tempdir().unwrap();
        let journal = directory.path().join("journal.json");
        {
            let mut policy = MediaPolicyEngine::open(&journal).unwrap();
            assert_eq!(
                policy
                    .prepare(
                        operation(7, VirtualMediaCommand::SetConnected { connected: true }),
                        MediaEligibility::verified_for_test_profile(),
                        observed(false, false),
                        1_500
                    )
                    .unwrap(),
                MediaDecision::Permit
            );
        }
        let socket = directory.path().join("absent.sock");
        let mut engine = MediaTransitionEngine::open(
            &journal,
            KvmdMediaClient::new(&socket).unwrap(),
            catalog(directory.path()),
            credentials(),
        )
        .unwrap();
        assert_eq!(engine.pending_count(), 1);
        assert_eq!(
            engine
                .execute(
                    operation(8, VirtualMediaCommand::SetConnected { connected: false }),
                    MediaEligibility::verified_for_test_profile(),
                    observed(false, true),
                    1_600
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::Rejected(MediaRejection::IndeterminatePending)
        );
        engine
            .resolve_pending([7; 16], MediaCompletion::Applied, 1_700)
            .unwrap();
        assert_eq!(engine.pending_count(), 0);
        assert_eq!(
            engine
                .execute(
                    operation(7, VirtualMediaCommand::SetConnected { connected: true }),
                    MediaEligibility::verified_for_test_profile(),
                    observed(false, true),
                    1_800
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::ReplayedApplied
        );
        assert!(!socket.exists());
    }

    #[tokio::test]
    async fn conflict_and_cooldown_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("media.sock");
        let server = serve(&socket, &["200 OK"]).await;
        let mut engine = engine(directory.path(), &socket);
        let eligibility = MediaEligibility::verified_for_test_profile();

        assert_eq!(
            engine
                .execute(
                    operation(9, VirtualMediaCommand::SetConnected { connected: true }),
                    eligibility,
                    observed(false, false),
                    1_500
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::Applied
        );
        assert_eq!(
            engine
                .execute(
                    operation(9, VirtualMediaCommand::SetConnected { connected: false }),
                    eligibility,
                    observed(false, true),
                    1_600
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::Rejected(MediaRejection::OperationIdConflict)
        );
        assert_eq!(
            engine
                .execute(
                    operation(10, VirtualMediaCommand::SetConnected { connected: false }),
                    eligibility,
                    observed(false, true),
                    1_600
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::Rejected(MediaRejection::CooldownActive)
        );
        assert_eq!(server.await.unwrap().len(), 1);
    }
}
