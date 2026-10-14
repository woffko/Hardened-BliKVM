use std::time::{Duration, Instant};

mod ledger;

pub use ledger::{HidReleaseLedger, HidReleaseLedgerError};

use bli_kvm_core::HidReport;
#[cfg(feature = "gadget-hid-profile")]
use bli_kvm_hid_adapter::{
    AbsolutePointerReportComposer, KeyboardReportComposer, RelativePointerReportComposer,
};
use bli_kvm_hid_adapter::{
    HidLeaseAcquisition, HidLeaseError, HidLeaseIdentity, HidLeaseManager, KvmdHidEvent,
};
#[cfg(feature = "kvmd-hid-profile")]
use bli_kvm_kvmd_client::KvmdCredentials;
#[cfg(feature = "kvmd-hid-profile")]
use bli_kvm_kvmd_hid_client::{KvmdHidClient, KvmdHidClientError};
#[cfg(feature = "gadget-hid-profile")]
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HidTransitionError {
    #[error("HID lease rejected: {0:?}")]
    Lease(HidLeaseError),
    #[error("HID transition backend failed")]
    Backend(#[source] HidExecutorError),
    #[error("HID release ledger failed")]
    Ledger(#[from] HidReleaseLedgerError),
}

/// Backend write failure, decoupled from the KVMD client so gadget builds do
/// not link KVMD.
#[derive(Debug, Error)]
pub enum HidExecutorError {
    #[cfg(feature = "kvmd-hid-profile")]
    #[error("KVMD HID backend failed")]
    Kvmd(#[from] KvmdHidClientError),
    #[error("HID backend failed: {0}")]
    Backend(String),
}

pub struct HidTransitionEngine {
    manager: HidLeaseManager,
    executor: HidExecutor,
    ledger: HidReleaseLedger,
    pending_releases: Vec<KvmdHidEvent>,
}

/// KVMD-backed HID executor. Holds its own credential instance.
#[cfg(feature = "kvmd-hid-profile")]
pub struct KvmdHidExecutor {
    client: KvmdHidClient,
    credentials: KvmdCredentials,
}

#[cfg(feature = "kvmd-hid-profile")]
impl KvmdHidExecutor {
    pub fn new(client: KvmdHidClient, credentials: KvmdCredentials) -> Self {
        Self {
            client,
            credentials,
        }
    }

    async fn execute_batch(&self, events: &[KvmdHidEvent]) -> Result<(), HidExecutorError> {
        self.client
            .execute_batch(events, &self.credentials)
            .await
            .map_err(HidExecutorError::Kvmd)
    }
}

/// Backend that writes HID report batches.
///
/// The transition engine is backend-agnostic so the KVMD executor used during
/// migration can later be replaced by the Rust USB gadget writer without
/// touching lease policy or broker wiring.
/// Configuration for the Rust gadget HID executor.
#[cfg(feature = "gadget-hid-profile")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GadgetHidConfig {
    pub hidg_dir: PathBuf,
    pub keyboard_function: String,
    pub absolute_mouse_function: String,
    pub relative_mouse_function: String,
    /// The configfs root of a gadget this owner builds. With it, every
    /// absolute mouse report follows `hid.usb1`'s present descriptor: report
    /// ID 1 while it carries the target agent's collection, none without, and
    /// nothing written for a descriptor that is neither. Without it the
    /// reports are the pinned descriptor's.
    pub configfs_root: Option<PathBuf>,
}

/// Which pinned pointer interface currently owns the pointer reports.
#[cfg(feature = "gadget-hid-profile")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PointerMode {
    /// KVMD's default mouse interface (`usb`, absolute).
    #[default]
    Absolute,
    /// The relative interface (`usb_rel`, enabled by `mouse_alt`).
    Relative,
}

/// Rust gadget HID executor: composes reports for the pinned boot keyboard and
/// both pointer interfaces and writes them to the function character devices.
///
/// Pointer reports follow KVMD semantics: a move selects the absolute
/// (`hid.usb1`) or relative (`hid.usb2`) interface, buttons and wheel follow the
/// last selected interface, and switching clears the interface left behind.
#[cfg(feature = "gadget-hid-profile")]
pub struct GadgetHidExecutor {
    config: GadgetHidConfig,
    keyboard: std::sync::Mutex<KeyboardReportComposer>,
    absolute: std::sync::Mutex<AbsolutePointerReportComposer>,
    relative: std::sync::Mutex<RelativePointerReportComposer>,
    mode: std::sync::Mutex<PointerMode>,
}

#[cfg(feature = "gadget-hid-profile")]
impl GadgetHidExecutor {
    pub fn new(config: GadgetHidConfig) -> Self {
        Self {
            config,
            keyboard: std::sync::Mutex::new(KeyboardReportComposer::new()),
            absolute: std::sync::Mutex::new(AbsolutePointerReportComposer::new()),
            relative: std::sync::Mutex::new(RelativePointerReportComposer::new()),
            mode: std::sync::Mutex::new(PointerMode::default()),
        }
    }

    async fn execute_batch(&self, events: &[KvmdHidEvent]) -> Result<(), HidExecutorError> {
        let mut keyboard = self
            .keyboard
            .lock()
            .map_err(|_| gadget_error("gadget HID keyboard state is unavailable"))?;
        let mut absolute = self
            .absolute
            .lock()
            .map_err(|_| gadget_error("gadget HID absolute pointer state is unavailable"))?;
        let mut relative = self
            .relative
            .lock()
            .map_err(|_| gadget_error("gadget HID relative pointer state is unavailable"))?;
        let mut mode = self
            .mode
            .lock()
            .map_err(|_| gadget_error("gadget HID pointer mode is unavailable"))?;
        for event in events {
            match keyboard.apply(event) {
                Ok(Some(report)) => self.write_keyboard(&report)?,
                Ok(None) => {
                    let selected = match event {
                        KvmdHidEvent::MouseMove { .. } => Some(PointerMode::Absolute),
                        KvmdHidEvent::MouseRelative { .. } => Some(PointerMode::Relative),
                        _ => None,
                    };
                    if let Some(next) = selected {
                        if next != *mode {
                            self.clear_pointer(*mode, &mut absolute, &mut relative)?;
                            *mode = next;
                        }
                    }
                    let unsupported = |_| {
                        gadget_error("gadget HID event is not supported by the pinned interfaces")
                    };
                    match *mode {
                        PointerMode::Absolute => match absolute.apply(event) {
                            Some(report) => self.write_absolute(&report)?,
                            None => return Err(unsupported(())),
                        },
                        PointerMode::Relative => match relative.apply(event) {
                            Some(report) => self.write_relative(&report)?,
                            None => return Err(unsupported(())),
                        },
                    }
                }
                Err(_) => return Err(gadget_error("gadget HID keyboard report is invalid")),
            }
        }
        Ok(())
    }

    fn write_keyboard(&self, report: &[u8; 8]) -> Result<(), HidExecutorError> {
        bli_kvm_usb_gadget::write_report(
            &self.config.hidg_dir,
            &self.config.keyboard_function,
            report,
        )
        .map_err(|_| gadget_error("gadget HID keyboard write failed"))
    }

    fn write_absolute(&self, report: &[u8; 7]) -> Result<(), HidExecutorError> {
        let report_id = match &self.config.configfs_root {
            Some(root) => bli_kvm_usb_gadget::absolute_mouse_report_id(root).map_err(|_| {
                gadget_error("gadget HID absolute pointer descriptor is not a known one")
            })?,
            None => None,
        };
        let mut bytes = Vec::with_capacity(8);
        bytes.extend(report_id);
        bytes.extend_from_slice(report);
        bli_kvm_usb_gadget::write_report(
            &self.config.hidg_dir,
            &self.config.absolute_mouse_function,
            &bytes,
        )
        .map_err(|_| gadget_error("gadget HID absolute pointer write failed"))
    }

    fn write_relative(&self, report: &[u8; 5]) -> Result<(), HidExecutorError> {
        bli_kvm_usb_gadget::write_report(
            &self.config.hidg_dir,
            &self.config.relative_mouse_function,
            report,
        )
        .map_err(|_| gadget_error("gadget HID relative pointer write failed"))
    }

    fn clear_pointer(
        &self,
        mode: PointerMode,
        absolute: &mut AbsolutePointerReportComposer,
        relative: &mut RelativePointerReportComposer,
    ) -> Result<(), HidExecutorError> {
        match mode {
            PointerMode::Absolute => {
                let report = absolute.release_all();
                self.write_absolute(&report)
            }
            PointerMode::Relative => {
                let report = relative.release_all();
                self.write_relative(&report)
            }
        }
    }
}

#[cfg(feature = "gadget-hid-profile")]
fn gadget_error(message: &str) -> HidExecutorError {
    HidExecutorError::Backend(message.to_string())
}

pub enum HidExecutor {
    #[cfg(feature = "kvmd-hid-profile")]
    Kvmd(KvmdHidExecutor),
    #[cfg(feature = "gadget-hid-profile")]
    Gadget(GadgetHidExecutor),
    #[cfg(test)]
    Test { fail: bool },
}

impl HidExecutor {
    async fn execute_batch(&self, events: &[KvmdHidEvent]) -> Result<(), HidExecutorError> {
        match self {
            #[cfg(feature = "kvmd-hid-profile")]
            HidExecutor::Kvmd(executor) => executor.execute_batch(events).await,
            #[cfg(feature = "gadget-hid-profile")]
            HidExecutor::Gadget(executor) => executor.execute_batch(events).await,
            #[cfg(test)]
            HidExecutor::Test { fail } => {
                if *fail {
                    Err(HidExecutorError::Backend(
                        "test backend failure".to_string(),
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }
}

impl HidTransitionEngine {
    #[cfg(feature = "kvmd-hid-profile")]
    pub fn new(
        client: KvmdHidClient,
        credentials: KvmdCredentials,
        ledger: HidReleaseLedger,
    ) -> Result<Self, HidTransitionError> {
        Self::new_with_executor(
            HidExecutor::Kvmd(KvmdHidExecutor::new(client, credentials)),
            ledger,
        )
    }

    /// Build an engine over any reviewed executor backend.
    pub fn new_with_executor(
        executor: HidExecutor,
        ledger: HidReleaseLedger,
    ) -> Result<Self, HidTransitionError> {
        let pending_releases = ledger.load()?;
        Ok(Self {
            manager: HidLeaseManager::new(),
            executor,
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

    pub async fn renew(
        &mut self,
        identity: HidLeaseIdentity,
        now: Instant,
        ttl: Duration,
    ) -> Result<(), HidTransitionError> {
        self.flush_pending().await?;
        let before = self.manager.release_snapshot();
        let batch = self.manager.renew(identity, now, ttl);
        let after = self.manager.release_snapshot();
        let lease_error = batch.error;
        self.execute_transition(&before, &batch.events, &after)
            .await?;
        match lease_error {
            Some(error) => Err(HidTransitionError::Lease(error)),
            None => Ok(()),
        }
    }

    /// Whether `identity` holds the lease now (see `HidLeaseManager::holds`).
    pub fn holds(&self, identity: HidLeaseIdentity, now: Instant) -> bool {
        self.manager.holds(identity, now)
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
        self.flush_pending().await?;
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
            && let Err(error) = self.executor.execute_batch(events).await
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
        self.executor
            .execute_batch(&self.pending_releases)
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
    async fn engine_executes_through_any_reviewed_backend() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = HidReleaseLedger::new(directory.path().join("ledger.json")).unwrap();
        let mut engine =
            HidTransitionEngine::new_with_executor(HidExecutor::Test { fail: false }, ledger)
                .unwrap();
        let owner = identity(1);
        let now = Instant::now();
        let key = HidReport::Keyboard {
            data: [0, 0, 4, 0, 0, 0, 0, 0],
        };
        engine
            .acquire(owner, now, Duration::from_secs(10))
            .await
            .unwrap();
        engine.report(owner, now, &key).await.unwrap();
        engine.release(owner).await.unwrap();
        assert_eq!(engine.pending_release_count(), 0);

        let ledger = HidReleaseLedger::new(directory.path().join("ledger-fail.json")).unwrap();
        let mut failing =
            HidTransitionEngine::new_with_executor(HidExecutor::Test { fail: true }, ledger)
                .unwrap();
        failing
            .acquire(owner, now, Duration::from_secs(10))
            .await
            .unwrap();
        assert!(matches!(
            failing.report(owner, now, &key).await,
            Err(HidTransitionError::Backend(_))
        ));
        assert!(failing.pending_release_count() > 0);
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
            engine.renew(owner, now, Duration::from_secs(10)).await,
            Err(HidTransitionError::Lease(HidLeaseError::NotActive))
        ));
    }

    #[tokio::test]
    async fn expired_renew_executes_release_before_returning_expired() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let server = serve_statuses(&socket, &["200 OK", "200 OK"]).await;
        let mut engine = engine(&socket, &directory.path().join("releases.json"));
        let owner = identity(1);
        let now = Instant::now();
        engine
            .acquire(owner, now, Duration::from_secs(2))
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
        assert!(matches!(
            engine
                .renew(owner, now + Duration::from_secs(2), Duration::from_secs(2))
                .await,
            Err(HidTransitionError::Lease(HidLeaseError::Expired))
        ));
        let requests = server.await.unwrap();
        assert!(requests[0].contains("key=KeyA&state=1"));
        assert!(requests[1].contains("key=KeyA&state=0"));
        assert_eq!(engine.pending_release_count(), 0);
    }

    #[tokio::test]
    async fn expiry_retries_a_release_after_the_backend_recovers() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let press_server = serve_statuses(&socket, &["200 OK"]).await;
        let mut engine = engine(&socket, &directory.path().join("releases.json"));
        let owner = identity(1);
        let now = Instant::now();
        engine
            .acquire(owner, now, Duration::from_secs(2))
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
        let requests = press_server.await.unwrap();
        assert!(requests[0].contains("key=KeyA&state=1"));
        fs::remove_file(&socket).unwrap();

        assert!(matches!(
            engine.expire(now + Duration::from_secs(2)).await,
            Err(HidTransitionError::Backend(_))
        ));
        assert_eq!(engine.pending_release_count(), 1);

        let recovery_server = serve_statuses(&socket, &["200 OK"]).await;
        engine.expire(now + Duration::from_secs(3)).await.unwrap();
        let requests = recovery_server.await.unwrap();
        assert!(requests[0].contains("key=KeyA&state=0"));
        assert_eq!(engine.pending_release_count(), 0);
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

    #[cfg(feature = "gadget-hid-profile")]
    fn last_report(directory: &std::path::Path, name: &str, length: usize) -> Vec<u8> {
        let bytes = std::fs::read(directory.join(name)).unwrap();
        assert!(bytes.len() >= length, "{name} has no report yet");
        bytes[bytes.len() - length..].to_vec()
    }

    #[cfg(feature = "gadget-hid-profile")]
    #[tokio::test]
    async fn gadget_executor_writes_boot_keyboard_reports() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("hidg0"), b"").unwrap();
        std::fs::write(directory.path().join("hidg1"), b"").unwrap();
        std::fs::write(directory.path().join("hidg2"), b"").unwrap();
        let ledger = HidReleaseLedger::new(directory.path().join("ledger.json")).unwrap();
        let mut engine = HidTransitionEngine::new_with_executor(
            HidExecutor::Gadget(GadgetHidExecutor::new(GadgetHidConfig {
                hidg_dir: directory.path().to_path_buf(),
                keyboard_function: "hid.usb0".into(),
                absolute_mouse_function: "hid.usb1".into(),
                relative_mouse_function: "hid.usb2".into(),
                configfs_root: None,
            })),
            ledger,
        )
        .unwrap();
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
        assert_eq!(
            last_report(directory.path(), "hidg0", 8),
            vec![0, 0, 4, 0, 0, 0, 0, 0]
        );
        engine.release(owner).await.unwrap();
        assert_eq!(last_report(directory.path(), "hidg0", 8), vec![0; 8]);
    }

    #[cfg(feature = "gadget-hid-profile")]
    #[tokio::test]
    async fn gadget_executor_writes_pinned_pointer_reports() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("hidg0"), b"").unwrap();
        std::fs::write(directory.path().join("hidg1"), b"").unwrap();
        std::fs::write(directory.path().join("hidg2"), b"").unwrap();
        let ledger = HidReleaseLedger::new(directory.path().join("ledger.json")).unwrap();
        let mut engine = HidTransitionEngine::new_with_executor(
            HidExecutor::Gadget(GadgetHidExecutor::new(GadgetHidConfig {
                hidg_dir: directory.path().to_path_buf(),
                keyboard_function: "hid.usb0".into(),
                absolute_mouse_function: "hid.usb1".into(),
                relative_mouse_function: "hid.usb2".into(),
                configfs_root: None,
            })),
            ledger,
        )
        .unwrap();
        let owner = identity(1);
        let now = Instant::now();
        engine
            .acquire(owner, now, Duration::from_secs(10))
            .await
            .unwrap();
        // KVMD absolute mouse: 16-bit X/Y remapped from the move range.
        engine
            .report(
                owner,
                now,
                &HidReport::AbsoluteMouse {
                    data: [0, 0x00, 0x00, 0xff, 0x7f, 0],
                },
            )
            .await
            .unwrap();
        assert_eq!(
            last_report(directory.path(), "hidg1", 7),
            vec![0, 0x00, 0x00, 0xff, 0x7f, 0, 0]
        );
        // A relative move selects hid.usb2 and clears the absolute interface.
        engine
            .report(owner, now, &HidReport::RelativeMouse { data: [0, 5, 0, 0] })
            .await
            .unwrap();
        // Switching releases the buttons but keeps the absolute position,
        // matching KVMD's clear event.
        assert_eq!(
            last_report(directory.path(), "hidg1", 7),
            vec![0, 0x00, 0x00, 0xff, 0x7f, 0, 0]
        );
        assert_eq!(
            last_report(directory.path(), "hidg2", 5),
            vec![0, 5, 0, 0, 0]
        );
    }

    /// One absolute move with `hid.usb1`'s descriptor as given (none: the
    /// file is missing): whether it was taken, and what reached the device.
    #[cfg(feature = "gadget-hid-profile")]
    async fn absolute_move_with(descriptor: Option<Vec<u8>>) -> (bool, Vec<u8>) {
        let directory = tempfile::tempdir().unwrap();
        let hidg = directory.path().join("dev");
        let function = directory
            .path()
            .join("configfs/hardened/functions/hid.usb1");
        std::fs::create_dir_all(&hidg).unwrap();
        std::fs::create_dir_all(&function).unwrap();
        std::fs::write(hidg.join("hidg1"), b"").unwrap();
        if let Some(descriptor) = descriptor {
            std::fs::write(function.join("report_desc"), descriptor).unwrap();
        }
        let ledger = HidReleaseLedger::new(directory.path().join("ledger.json")).unwrap();
        let mut engine = HidTransitionEngine::new_with_executor(
            HidExecutor::Gadget(GadgetHidExecutor::new(GadgetHidConfig {
                hidg_dir: hidg.clone(),
                keyboard_function: "hid.usb0".into(),
                absolute_mouse_function: "hid.usb1".into(),
                relative_mouse_function: "hid.usb2".into(),
                configfs_root: Some(directory.path().join("configfs")),
            })),
            ledger,
        )
        .unwrap();
        let owner = identity(1);
        let now = Instant::now();
        engine
            .acquire(owner, now, Duration::from_secs(10))
            .await
            .unwrap();
        let taken = engine
            .report(
                owner,
                now,
                &HidReport::AbsoluteMouse {
                    data: [1, 0x00, 0x40, 0x00, 0x20, 0],
                },
            )
            .await
            .is_ok();
        (taken, std::fs::read(hidg.join("hidg1")).unwrap())
    }

    #[cfg(feature = "gadget-hid-profile")]
    #[tokio::test]
    async fn absolute_reports_follow_the_present_descriptor_of_the_mouse() {
        let descriptor =
            |agent: bool| bli_kvm_usb_gadget::absolute_mouse_function(agent).report_descriptor;
        // The pinned descriptor: the 7 bytes as without a gadget of our own.
        let (taken, plain) = absolute_move_with(Some(descriptor(false))).await;
        assert!(taken);
        assert_eq!(plain.len(), 7);
        assert_eq!(plain[0], 1, "the left button");
        // With the agent's collection: the same report after its ID 1.
        let (taken, with_id) = absolute_move_with(Some(descriptor(true))).await;
        assert!(taken);
        assert_eq!(with_id[0], 1);
        assert_eq!(&with_id[1..], &plain[..]);
        // A descriptor that is neither, or none: nothing is written.
        assert_eq!(
            absolute_move_with(Some(vec![0x05, 0x01])).await,
            (false, vec![])
        );
        assert_eq!(absolute_move_with(None).await, (false, vec![]));
    }
}
