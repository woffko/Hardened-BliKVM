use std::time::{Duration, Instant};

mod ledger;

pub use ledger::{HidReleaseLedger, HidReleaseLedgerError};

use bli_kvm_core::HidReport;
use bli_kvm_hid_adapter::{
    HidLeaseAcquisition, HidLeaseError, HidLeaseIdentity, HidLeaseManager, KvmdHidEvent,
};
use bli_kvm_kvmd_client::KvmdCredentials;
use bli_kvm_kvmd_hid_client::{KvmdHidClient, KvmdHidClientError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HidTransitionError {
    #[error("HID lease rejected: {0:?}")]
    Lease(HidLeaseError),
    #[error("HID transition backend failed")]
    Backend(#[source] KvmdHidClientError),
    #[error("HID release ledger failed")]
    Ledger(#[from] HidReleaseLedgerError),
}

pub struct HidTransitionEngine {
    manager: HidLeaseManager,
    client: KvmdHidClient,
    credentials: KvmdCredentials,
    ledger: HidReleaseLedger,
    pending_releases: Vec<KvmdHidEvent>,
}

impl HidTransitionEngine {
    pub fn new(
        client: KvmdHidClient,
        credentials: KvmdCredentials,
        ledger: HidReleaseLedger,
    ) -> Result<Self, HidTransitionError> {
        let pending_releases = ledger.load()?;
        Ok(Self {
            manager: HidLeaseManager::new(),
            client,
            credentials,
            ledger,
            pending_releases,
        })
    }

    pub fn pending_release_count(&self) -> usize {
        self.pending_releases.len()
    }

    pub async fn acquire(
        &mut self,
        identity: HidLeaseIdentity,
        now: Instant,
        ttl: Duration,
    ) -> Result<(), HidTransitionError> {
        self.flush_pending().await?;
        let before = self.manager.release_snapshot();
        let HidLeaseAcquisition { release_events } = self
            .manager
            .acquire(identity, now, ttl)
            .map_err(HidTransitionError::Lease)?;
        let after = self.manager.release_snapshot();
        self.execute_transition(&before, &release_events, &after)
            .await
    }

    pub fn renew(
        &mut self,
        identity: HidLeaseIdentity,
        now: Instant,
        ttl: Duration,
    ) -> Result<(), HidTransitionError> {
        let batch = self.manager.renew(identity, now, ttl);
        match batch.error {
            Some(error) => Err(HidTransitionError::Lease(error)),
            None => Ok(()),
        }
    }

    pub async fn report(
        &mut self,
        identity: HidLeaseIdentity,
        now: Instant,
        report: &HidReport,
    ) -> Result<(), HidTransitionError> {
        let before = self.manager.release_snapshot();
        let batch = self.manager.translate(identity, now, report);
        let after = self.manager.release_snapshot();
        let lease_error = batch.error;
        self.execute_transition(&before, &batch.events, &after)
            .await?;
        match lease_error {
            Some(error) => Err(HidTransitionError::Lease(error)),
            None => Ok(()),
        }
    }

    pub async fn release(&mut self, identity: HidLeaseIdentity) -> Result<(), HidTransitionError> {
        let before = self.manager.release_snapshot();
        let batch = self.manager.release(identity);
        let after = self.manager.release_snapshot();
        let lease_error = batch.error;
        self.execute_transition(&before, &batch.events, &after)
            .await?;
        match lease_error {
            Some(error) => Err(HidTransitionError::Lease(error)),
            None => Ok(()),
        }
    }

    pub async fn revoke_session(&mut self, session_binding: u64) -> Result<(), HidTransitionError> {
        let before = self.manager.release_snapshot();
        let events = self.manager.revoke_session(session_binding);
        let after = self.manager.release_snapshot();
        self.execute_transition(&before, &events, &after).await
    }

    pub async fn expire(&mut self, now: Instant) -> Result<(), HidTransitionError> {
        let before = self.manager.release_snapshot();
        let events = self.manager.expire(now);
        let after = self.manager.release_snapshot();
        self.execute_transition(&before, &events, &after).await
    }

    pub async fn disconnect(&mut self) -> Result<(), HidTransitionError> {
        self.flush_pending().await?;
        let before = self.manager.release_snapshot();
        let events = self.manager.disconnect();
        self.execute_transition(&before, &events, &[]).await
    }

    async fn execute_transition(
        &mut self,
        before: &[KvmdHidEvent],
        events: &[KvmdHidEvent],
        after: &[KvmdHidEvent],
    ) -> Result<(), HidTransitionError> {
        let state_changed = before != after;
        let union = merge_releases(before, after);
        if state_changed {
            if let Err(error) = self.ledger.store(&union) {
                let _ = self.manager.disconnect();
                self.pending_releases = merge_releases(&self.pending_releases, before);
                return Err(HidTransitionError::Ledger(error));
            }
        }
        if !events.is_empty()
            && let Err(error) = self.client.execute_batch(events, &self.credentials).await
        {
            self.remember_releases(events);
            let _ = self.ledger.store(&self.pending_releases);
            let _ = self.flush_pending_inner().await;
            return Err(HidTransitionError::Backend(error));
        }
        if state_changed && let Err(error) = self.ledger.store(after) {
            let _ = self.manager.disconnect();
            self.pending_releases = merge_releases(&self.pending_releases, &union);
            return Err(HidTransitionError::Ledger(error));
        }
        Ok(())
    }

    async fn flush_pending(&mut self) -> Result<(), HidTransitionError> {
        self.flush_pending_inner().await
    }

    async fn flush_pending_inner(&mut self) -> Result<(), HidTransitionError> {
        if self.pending_releases.is_empty() {
            return Ok(());
        }
        self.client
            .execute_batch(&self.pending_releases, &self.credentials)
            .await
            .map_err(HidTransitionError::Backend)?;
        self.ledger.store(&[])?;
        self.pending_releases.clear();
        Ok(())
    }

    fn remember_releases(&mut self, attempted: &[KvmdHidEvent]) {
        let mut releases = attempted
            .iter()
            .filter(|event| is_release(event))
            .cloned()
            .collect::<Vec<_>>();
        releases.extend(self.manager.disconnect());
        for event in releases {
            if !self.pending_releases.contains(&event) {
                self.pending_releases.push(event);
            }
        }
    }
}

fn merge_releases(left: &[KvmdHidEvent], right: &[KvmdHidEvent]) -> Vec<KvmdHidEvent> {
    let mut merged = Vec::new();
    for event in left.iter().chain(right).filter(|event| is_release(event)) {
        if !merged.contains(event) {
            merged.push(event.clone());
        }
    }
    merged
}

fn is_release(event: &KvmdHidEvent) -> bool {
    matches!(
        event,
        KvmdHidEvent::Key { pressed: false, .. } | KvmdHidEvent::MouseButton { pressed: false, .. }
    )
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
    };

    use super::*;

    fn identity(seed: u8) -> HidLeaseIdentity {
        HidLeaseIdentity::new([seed; 16], u64::from(seed), [seed; 32]).unwrap()
    }

    fn credentials() -> KvmdCredentials {
        KvmdCredentials::new("test-user", "not-a-real-password").unwrap()
    }

    fn engine(socket: &Path, ledger_path: &Path) -> HidTransitionEngine {
        HidTransitionEngine::new(
            KvmdHidClient::new(socket).unwrap(),
            credentials(),
            HidReleaseLedger::new(ledger_path).unwrap(),
        )
        .unwrap()
    }

    async fn serve_statuses(
        socket: &Path,
        statuses: &[&str],
    ) -> tokio::task::JoinHandle<Vec<String>> {
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

    #[tokio::test]
    async fn lease_bound_report_and_release_reach_only_fixed_key_routes() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let server = serve_statuses(&socket, &["200 OK", "200 OK"]).await;
        let mut engine = engine(&socket, &directory.path().join("releases.json"));
        let owner = identity(1);
        let now = Instant::now();
        engine
            .acquire(owner, now, Duration::from_secs(10))
            .await
            .unwrap();
        engine
            .report(
                owner,
                now,
                &HidReport::Keyboard {
                    data: [0, 0, 4, 0, 0, 0, 0, 0],
                },
            )
            .await
            .unwrap();
        engine.release(owner).await.unwrap();
        let requests = server.await.unwrap();
        assert!(requests[0].starts_with("POST /hid/events/send_key?key=KeyA&state=1 HTTP/1.1\r\n"));
        assert!(requests[1].starts_with("POST /hid/events/send_key?key=KeyA&state=0 HTTP/1.1\r\n"));
        assert_eq!(engine.pending_release_count(), 0);
    }

    #[tokio::test]
    async fn failed_press_attempts_release_and_revokes_the_lease() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let server = serve_statuses(&socket, &["500 Error", "200 OK"]).await;
        let mut engine = engine(&socket, &directory.path().join("releases.json"));
        let owner = identity(1);
        let now = Instant::now();
        engine
            .acquire(owner, now, Duration::from_secs(10))
            .await
            .unwrap();
        assert!(matches!(
            engine
                .report(
                    owner,
                    now,
                    &HidReport::Keyboard {
                        data: [0, 0, 4, 0, 0, 0, 0, 0]
                    }
                )
                .await,
            Err(HidTransitionError::Backend(_))
        ));
        let requests = server.await.unwrap();
        assert!(requests[0].contains("key=KeyA&state=1"));
        assert!(requests[1].contains("key=KeyA&state=0"));
        assert_eq!(engine.pending_release_count(), 0);
        assert!(matches!(
            engine.renew(owner, now, Duration::from_secs(10)),
            Err(HidTransitionError::Lease(HidLeaseError::NotActive))
        ));
    }

    #[tokio::test]
    async fn pending_release_blocks_new_lease_until_backend_recovers() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let mut engine = engine(&socket, &directory.path().join("releases.json"));
        let first = identity(1);
        let now = Instant::now();
        engine
            .acquire(first, now, Duration::from_secs(10))
            .await
            .unwrap();
        assert!(matches!(
            engine
                .report(
                    first,
                    now,
                    &HidReport::Keyboard {
                        data: [0, 0, 4, 0, 0, 0, 0, 0]
                    }
                )
                .await,
            Err(HidTransitionError::Backend(_))
        ));
        assert_eq!(engine.pending_release_count(), 1);

        let server = serve_statuses(&socket, &["200 OK"]).await;
        engine
            .acquire(identity(2), now, Duration::from_secs(10))
            .await
            .unwrap();
        let requests = server.await.unwrap();
        assert!(requests[0].contains("key=KeyA&state=0"));
        assert_eq!(engine.pending_release_count(), 0);
    }

    #[tokio::test]
    async fn mismatched_lease_is_rejected_without_backend_io() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("missing-kvmd.sock");
        let mut engine = engine(&socket, &directory.path().join("releases.json"));
        let now = Instant::now();
        engine
            .acquire(identity(1), now, Duration::from_secs(10))
            .await
            .unwrap();
        assert!(matches!(
            engine
                .report(identity(2), now, &HidReport::Keyboard { data: [0; 8] })
                .await,
            Err(HidTransitionError::Lease(HidLeaseError::Mismatch))
        ));
    }

    #[tokio::test]
    async fn restart_replays_persisted_release_before_new_lease() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let ledger_path = directory.path().join("releases.json");
        let first_server = serve_statuses(&socket, &["200 OK"]).await;
        let now = Instant::now();
        {
            let mut first_engine = engine(&socket, &ledger_path);
            let owner = identity(1);
            first_engine
                .acquire(owner, now, Duration::from_secs(10))
                .await
                .unwrap();
            first_engine
                .report(
                    owner,
                    now,
                    &HidReport::Keyboard {
                        data: [0, 0, 4, 0, 0, 0, 0, 0],
                    },
                )
                .await
                .unwrap();
        }
        let first_requests = first_server.await.unwrap();
        assert!(first_requests[0].contains("key=KeyA&state=1"));
        fs::remove_file(&socket).unwrap();
        assert_eq!(
            HidReleaseLedger::new(&ledger_path)
                .unwrap()
                .load()
                .unwrap()
                .len(),
            1
        );

        let recovery_server = serve_statuses(&socket, &["200 OK"]).await;
        let mut recovered = engine(&socket, &ledger_path);
        assert_eq!(recovered.pending_release_count(), 1);
        recovered
            .acquire(identity(2), now, Duration::from_secs(10))
            .await
            .unwrap();
        let recovery_requests = recovery_server.await.unwrap();
        assert!(recovery_requests[0].contains("key=KeyA&state=0"));
        assert_eq!(recovered.pending_release_count(), 0);
        assert!(
            HidReleaseLedger::new(&ledger_path)
                .unwrap()
                .load()
                .unwrap()
                .is_empty()
        );
    }
}
