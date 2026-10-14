//! Eligibility-gated HID control policy for the restricted broker.
//!
//! The policy never enables the HID capability by itself. Production builds can
//! only construct [`HidEligibility::disabled`], so every lease mutation is
//! rejected with `capability_disabled` and no backend socket is opened. A
//! separate test profile may construct an enabled eligibility token. Safety
//! operations (release, session revocation, expiry and disconnect) are never
//! gated, so held keys and buttons are always released.

use std::time::{Duration, Instant};

use bli_kvm_control_protocol::ControlRejectionCode;
use bli_kvm_core::HidReport;
use bli_kvm_hid_adapter::{HidLeaseError, HidLeaseIdentity, MAX_HID_LEASE_TTL};
use bli_kvm_hid_transition::{HidTransitionEngine, HidTransitionError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HidEligibility {
    capability_enabled: bool,
    backend_ready: bool,
    attestation_verified: bool,
}

impl HidEligibility {
    pub const fn disabled() -> Self {
        Self {
            capability_enabled: false,
            backend_ready: false,
            attestation_verified: false,
        }
    }

    #[cfg(feature = "hid-test-profile")]
    pub const fn verified_for_test_profile() -> Self {
        Self::verified()
    }

    #[cfg(test)]
    const fn verified_for_test() -> Self {
        Self::verified()
    }

    #[cfg(any(test, feature = "hid-test-profile"))]
    const fn verified() -> Self {
        Self {
            capability_enabled: true,
            backend_ready: true,
            attestation_verified: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidDecision {
    Applied,
    Reject(HidRejection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidRejection {
    CapabilityDisabled,
    BackendUnavailable,
    ConfirmationInvalid,
    InvalidTtl,
    LeaseConflict,
    LeaseExpired,
    LeaseMismatch,
    InvalidReport,
    Internal,
}

impl HidRejection {
    pub const fn rejection_code(self) -> ControlRejectionCode {
        match self {
            Self::CapabilityDisabled => ControlRejectionCode::CapabilityDisabled,
            Self::BackendUnavailable | Self::Internal => ControlRejectionCode::BackendUnavailable,
            Self::ConfirmationInvalid => ControlRejectionCode::NotAttested,
            Self::InvalidTtl | Self::InvalidReport => ControlRejectionCode::InvalidState,
            Self::LeaseConflict | Self::LeaseMismatch => ControlRejectionCode::LeaseConflict,
            Self::LeaseExpired => ControlRejectionCode::LeaseExpired,
        }
    }
}

pub trait HidDiagnosticSink {
    /// Reports a fixed rejection. The returned flag is advisory and must never
    /// change the control decision.
    fn record(&self, rejection: HidRejection) -> bool;
}

pub fn report_rejection<S: HidDiagnosticSink>(decision: HidDecision, sink: &S) -> HidDecision {
    if let HidDecision::Reject(rejection) = decision {
        let _ = sink.record(rejection);
    }
    decision
}

pub struct HidControlPolicy {
    engine: HidTransitionEngine,
}

impl HidControlPolicy {
    pub fn new(engine: HidTransitionEngine) -> Self {
        Self { engine }
    }

    pub fn pending_release_count(&self) -> usize {
        self.engine.pending_release_count()
    }

    pub async fn acquire(
        &mut self,
        identity: HidLeaseIdentity,
        eligibility: HidEligibility,
        now: Instant,
        ttl: Duration,
    ) -> HidDecision {
        if let Some(rejection) = mutation_gate(eligibility, ttl) {
            return HidDecision::Reject(rejection);
        }
        // The broker admits one lease at a time. A client that lost its lease
        // id must always be able to take its own session's lease back, so on a
        // conflict the requester's own session is revoked and the acquire is
        // retried once. A conflict with a different session still fails closed.
        match self.engine.acquire(identity, now, ttl).await {
            Err(HidTransitionError::Lease(HidLeaseError::Busy)) => {
                let _ = self.engine.revoke_session(identity.session_binding()).await;
                self.engine
                    .acquire(identity, now, ttl)
                    .await
                    .map_or_else(map_error, |()| HidDecision::Applied)
            }
            result => result.map_or_else(map_error, |()| HidDecision::Applied),
        }
    }

    pub async fn renew(
        &mut self,
        identity: HidLeaseIdentity,
        eligibility: HidEligibility,
        now: Instant,
        ttl: Duration,
    ) -> HidDecision {
        if let Some(rejection) = mutation_gate(eligibility, ttl) {
            return HidDecision::Reject(rejection);
        }
        self.engine
            .renew(identity, now, ttl)
            .await
            .map_or_else(map_error, |()| HidDecision::Applied)
    }

    pub async fn report(
        &mut self,
        identity: HidLeaseIdentity,
        eligibility: HidEligibility,
        now: Instant,
        report: &HidReport,
    ) -> HidDecision {
        if let Some(rejection) = mutation_gate(eligibility, MAX_HID_LEASE_TTL) {
            return HidDecision::Reject(rejection);
        }
        self.engine
            .report(identity, now, report)
            .await
            .map_or_else(map_error, |()| HidDecision::Applied)
    }

    /// Whether `identity` holds the lease now.
    pub fn holds(&self, identity: HidLeaseIdentity, now: Instant) -> bool {
        self.engine.holds(identity, now)
    }

    pub async fn release(&mut self, identity: HidLeaseIdentity) -> HidDecision {
        match self.engine.release(identity).await {
            Ok(()) => HidDecision::Applied,
            Err(HidTransitionError::Lease(HidLeaseError::NotActive)) => HidDecision::Applied,
            Err(error) => map_error(error),
        }
    }

    pub async fn revoke_session(&mut self, session_binding: u64) -> HidDecision {
        self.engine
            .revoke_session(session_binding)
            .await
            .map_or_else(map_error, |()| HidDecision::Applied)
    }

    pub async fn expire(&mut self, now: Instant) -> HidDecision {
        self.engine
            .expire(now)
            .await
            .map_or_else(map_error, |()| HidDecision::Applied)
    }

    pub async fn disconnect(&mut self) -> HidDecision {
        self.engine
            .disconnect()
            .await
            .map_or_else(map_error, |()| HidDecision::Applied)
    }
}

fn mutation_gate(eligibility: HidEligibility, ttl: Duration) -> Option<HidRejection> {
    if !eligibility.capability_enabled {
        return Some(HidRejection::CapabilityDisabled);
    }
    if !eligibility.backend_ready {
        return Some(HidRejection::BackendUnavailable);
    }
    if !eligibility.attestation_verified {
        return Some(HidRejection::ConfirmationInvalid);
    }
    if ttl.is_zero() || ttl > MAX_HID_LEASE_TTL {
        return Some(HidRejection::InvalidTtl);
    }
    None
}

fn map_error(error: HidTransitionError) -> HidDecision {
    let rejection = match error {
        HidTransitionError::Lease(HidLeaseError::Busy) => HidRejection::LeaseConflict,
        HidTransitionError::Lease(HidLeaseError::Expired) => HidRejection::LeaseExpired,
        HidTransitionError::Lease(
            HidLeaseError::InvalidIdentity
            | HidLeaseError::InvalidTtl
            | HidLeaseError::Mismatch
            | HidLeaseError::NotActive,
        ) => HidRejection::LeaseMismatch,
        HidTransitionError::Lease(HidLeaseError::Translation(_)) => HidRejection::InvalidReport,
        HidTransitionError::Backend(_) | HidTransitionError::Ledger(_) => {
            HidRejection::BackendUnavailable
        }
    };
    HidDecision::Reject(rejection)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use bli_kvm_hid_transition::HidReleaseLedger;
    use bli_kvm_kvmd_client::KvmdCredentials;
    use bli_kvm_kvmd_hid_client::KvmdHidClient;

    use super::*;

    fn identity(seed: u8) -> HidLeaseIdentity {
        HidLeaseIdentity::new([seed; 16], u64::from(seed), [seed; 32]).unwrap()
    }

    fn policy(ledger_path: &Path, socket: &Path) -> HidControlPolicy {
        HidControlPolicy::new(
            HidTransitionEngine::new(
                KvmdHidClient::new(socket).unwrap(),
                KvmdCredentials::new("test-user", "not-a-real-password").unwrap(),
                HidReleaseLedger::new(ledger_path).unwrap(),
            )
            .unwrap(),
        )
    }

    fn unreachable_socket(directory: &Path) -> std::path::PathBuf {
        directory.join("absent.sock")
    }

    fn keyboard(report_data: [u8; 8]) -> HidReport {
        HidReport::Keyboard { data: report_data }
    }

    #[test]
    fn disabled_production_eligibility_is_the_default() {
        let disabled = HidEligibility::disabled();
        assert!(!disabled.capability_enabled);
        assert!(!disabled.backend_ready);
        assert!(!disabled.attestation_verified);
    }

    #[test]
    fn rejection_codes_are_fixed_and_typed() {
        assert_eq!(
            HidRejection::CapabilityDisabled.rejection_code(),
            ControlRejectionCode::CapabilityDisabled
        );
        assert_eq!(
            HidRejection::ConfirmationInvalid.rejection_code(),
            ControlRejectionCode::NotAttested
        );
        assert_eq!(
            HidRejection::LeaseConflict.rejection_code(),
            ControlRejectionCode::LeaseConflict
        );
        assert_eq!(
            HidRejection::LeaseExpired.rejection_code(),
            ControlRejectionCode::LeaseExpired
        );
    }

    #[tokio::test]
    async fn disabled_eligibility_rejects_mutations_without_contacting_a_backend() {
        let directory = tempfile::tempdir().unwrap();
        let socket = unreachable_socket(directory.path());
        let mut policy = policy(&directory.path().join("ledger.json"), &socket);
        let now = Instant::now();

        for decision in [
            policy
                .acquire(
                    identity(1),
                    HidEligibility::disabled(),
                    now,
                    Duration::from_secs(30),
                )
                .await,
            policy
                .renew(
                    identity(1),
                    HidEligibility::disabled(),
                    now,
                    Duration::from_secs(30),
                )
                .await,
            policy
                .report(
                    identity(1),
                    HidEligibility::disabled(),
                    now,
                    &keyboard([0, 0, 4, 0, 0, 0, 0, 0]),
                )
                .await,
        ] {
            assert_eq!(
                decision,
                HidDecision::Reject(HidRejection::CapabilityDisabled)
            );
        }
        assert!(!socket.exists());
    }

    #[tokio::test]
    async fn enabled_eligibility_still_requires_a_ready_backend_and_attestation() {
        let directory = tempfile::tempdir().unwrap();
        let socket = unreachable_socket(directory.path());
        let mut policy = policy(&directory.path().join("ledger.json"), &socket);
        let now = Instant::now();

        let ready = HidEligibility::verified_for_test();
        let mut backend_down = ready;
        backend_down.backend_ready = false;
        let mut unattested = ready;
        unattested.attestation_verified = false;

        assert_eq!(
            policy
                .acquire(identity(1), backend_down, now, Duration::from_secs(30))
                .await,
            HidDecision::Reject(HidRejection::BackendUnavailable)
        );
        assert_eq!(
            policy
                .acquire(identity(1), unattested, now, Duration::from_secs(30))
                .await,
            HidDecision::Reject(HidRejection::ConfirmationInvalid)
        );
        assert_eq!(
            policy
                .acquire(
                    identity(1),
                    ready,
                    now,
                    MAX_HID_LEASE_TTL + Duration::from_secs(1)
                )
                .await,
            HidDecision::Reject(HidRejection::InvalidTtl)
        );
        assert!(!socket.exists());
    }

    #[tokio::test]
    async fn safety_release_paths_work_even_when_the_capability_is_disabled() {
        let directory = tempfile::tempdir().unwrap();
        let socket = unreachable_socket(directory.path());
        let mut policy = policy(&directory.path().join("ledger.json"), &socket);
        let now = Instant::now();

        assert_eq!(policy.release(identity(1)).await, HidDecision::Applied);
        assert_eq!(policy.revoke_session(1).await, HidDecision::Applied);
        assert_eq!(policy.expire(now).await, HidDecision::Applied);
        assert_eq!(policy.disconnect().await, HidDecision::Applied);
        assert!(!socket.exists());
    }

    #[tokio::test]
    async fn a_persisted_release_is_visible_before_any_lease_is_granted() {
        let directory = tempfile::tempdir().unwrap();
        let ledger_path = directory.path().join("ledger.json");
        {
            let ledger = HidReleaseLedger::new(&ledger_path).unwrap();
            ledger
                .store(&[bli_kvm_hid_adapter::KvmdHidEvent::Key {
                    key: bli_kvm_hid_adapter::KvmdKeyCode::from_usb_usage(4).unwrap(),
                    pressed: false,
                }])
                .unwrap();
        }
        let policy = policy(&ledger_path, &unreachable_socket(directory.path()));
        assert_eq!(policy.pending_release_count(), 1);
    }

    async fn serve_statuses(
        socket: &Path,
        statuses: &[&str],
    ) -> tokio::task::JoinHandle<Vec<String>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::UnixListener;

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
    async fn enabled_profile_reaches_fixed_routes_and_releases_held_keys() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let server = serve_statuses(&socket, &["200 OK", "200 OK"]).await;
        let mut policy = policy(&directory.path().join("ledger.json"), &socket);
        let owner = identity(1);
        let now = Instant::now();
        let eligible = HidEligibility::verified_for_test();

        assert_eq!(
            policy
                .acquire(owner, eligible, now, Duration::from_secs(30))
                .await,
            HidDecision::Applied
        );
        assert_eq!(
            policy
                .report(owner, eligible, now, &keyboard([0, 0, 4, 0, 0, 0, 0, 0]))
                .await,
            HidDecision::Applied
        );
        assert_eq!(policy.release(owner).await, HidDecision::Applied);
        let requests = server.await.unwrap();
        assert!(requests[0].starts_with("POST /hid/events/send_key?key=KeyA&state=1 HTTP/1.1\r\n"));
        assert!(requests[1].starts_with("POST /hid/events/send_key?key=KeyA&state=0 HTTP/1.1\r\n"));
        assert_eq!(policy.pending_release_count(), 0);
    }

    #[tokio::test]
    async fn failed_report_revokes_the_lease_and_completes_release_all() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let server = serve_statuses(&socket, &["500 Error", "200 OK"]).await;
        let mut policy = policy(&directory.path().join("ledger.json"), &socket);
        let owner = identity(1);
        let now = Instant::now();
        let eligible = HidEligibility::verified_for_test();

        assert_eq!(
            policy
                .acquire(owner, eligible, now, Duration::from_secs(30))
                .await,
            HidDecision::Applied
        );
        assert_eq!(
            policy
                .report(owner, eligible, now, &keyboard([0, 0, 4, 0, 0, 0, 0, 0]))
                .await,
            HidDecision::Reject(HidRejection::BackendUnavailable)
        );
        let requests = server.await.unwrap();
        assert!(requests[0].contains("key=KeyA&state=1"));
        assert!(requests[1].contains("key=KeyA&state=0"));
        assert_eq!(policy.pending_release_count(), 0);
        assert_eq!(policy.disconnect().await, HidDecision::Applied);
    }

    #[tokio::test]
    async fn startup_replays_a_persisted_release_before_any_new_lease() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let ledger_path = directory.path().join("ledger.json");
        {
            let ledger = HidReleaseLedger::new(&ledger_path).unwrap();
            ledger
                .store(&[bli_kvm_hid_adapter::KvmdHidEvent::Key {
                    key: bli_kvm_hid_adapter::KvmdKeyCode::from_usb_usage(4).unwrap(),
                    pressed: false,
                }])
                .unwrap();
        }
        let server = serve_statuses(&socket, &["200 OK"]).await;
        let mut policy = policy(&ledger_path, &socket);
        assert_eq!(policy.pending_release_count(), 1);
        assert_eq!(policy.disconnect().await, HidDecision::Applied);
        let requests = server.await.unwrap();
        assert!(requests[0].contains("key=KeyA&state=0"));
        assert_eq!(policy.pending_release_count(), 0);
    }

    #[tokio::test]
    async fn a_lost_lease_id_never_locks_the_same_session_out() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        // The supersede path may issue a release and a fresh acquire, so the
        // fake front answers generously and the test does not count requests.
        let _server = serve_statuses(&socket, &["200 OK", "200 OK", "200 OK", "200 OK"]).await;
        let mut policy = policy(&directory.path().join("ledger.json"), &socket);
        let session = identity(1);
        let now = Instant::now();
        let eligible = HidEligibility::verified_for_test();

        assert_eq!(
            policy
                .acquire(session, eligible, now, Duration::from_secs(30))
                .await,
            HidDecision::Applied
        );
        // The client lost its lease id and asks again: its own stale lease must
        // not lock it out, while another session still conflicts.
        assert_eq!(
            policy
                .acquire(session, eligible, now, Duration::from_secs(30))
                .await,
            HidDecision::Applied
        );
        assert_eq!(
            policy
                .acquire(identity(2), eligible, now, Duration::from_secs(30))
                .await,
            HidDecision::Reject(HidRejection::LeaseConflict)
        );
    }

    #[tokio::test]
    async fn expiry_releases_held_keys_through_the_fixed_route() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let server = serve_statuses(&socket, &["200 OK", "200 OK"]).await;
        let mut policy = policy(&directory.path().join("ledger.json"), &socket);
        let owner = identity(1);
        let now = Instant::now();
        let eligible = HidEligibility::verified_for_test();

        assert_eq!(
            policy
                .acquire(owner, eligible, now, Duration::from_secs(30))
                .await,
            HidDecision::Applied
        );
        assert_eq!(
            policy
                .report(owner, eligible, now, &keyboard([0, 0, 4, 0, 0, 0, 0, 0]))
                .await,
            HidDecision::Applied
        );
        assert_eq!(
            policy.expire(now + Duration::from_secs(31)).await,
            HidDecision::Applied
        );
        let requests = server.await.unwrap();
        assert!(requests[0].contains("key=KeyA&state=1"));
        assert!(requests[1].contains("key=KeyA&state=0"));
        assert_eq!(policy.pending_release_count(), 0);
    }

    #[tokio::test]
    async fn mismatched_identity_is_rejected_without_backend_io() {
        let directory = tempfile::tempdir().unwrap();
        let socket = unreachable_socket(directory.path());
        let mut policy = policy(&directory.path().join("ledger.json"), &socket);
        let now = Instant::now();
        let eligible = HidEligibility::verified_for_test();

        assert_eq!(
            policy
                .acquire(identity(1), eligible, now, Duration::from_secs(30))
                .await,
            HidDecision::Applied
        );
        assert_eq!(
            policy
                .report(
                    identity(2),
                    eligible,
                    now,
                    &keyboard([0, 0, 4, 0, 0, 0, 0, 0])
                )
                .await,
            HidDecision::Reject(HidRejection::LeaseMismatch)
        );
        assert_eq!(
            policy
                .renew(identity(2), eligible, now, Duration::from_secs(30))
                .await,
            HidDecision::Reject(HidRejection::LeaseMismatch)
        );
        assert!(!socket.exists());
    }

    struct ReportingSink {
        accepted: bool,
    }

    impl HidDiagnosticSink for ReportingSink {
        fn record(&self, _rejection: HidRejection) -> bool {
            self.accepted
        }
    }

    #[test]
    fn diagnostic_reporting_cannot_change_the_decision() {
        let decision = HidDecision::Reject(HidRejection::LeaseExpired);
        let accepted = ReportingSink { accepted: true };
        let rejected = ReportingSink { accepted: false };
        assert_eq!(report_rejection(decision, &accepted), decision);
        assert_eq!(report_rejection(decision, &rejected), decision);
        assert_eq!(
            report_rejection(HidDecision::Applied, &rejected),
            HidDecision::Applied
        );
    }
}
