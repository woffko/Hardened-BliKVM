use std::{io::Read, path::PathBuf};

use bli_kvm_update_manager::{
    ActivationOutcome, ActivationRequest, ApplicationUpdateManager, ConfirmationOutcome,
    ManagerStatus, RecoveryOutcome, UpdateManagerError,
};
use zeroize::Zeroize;

use crate::{HealthChallengeStatus, HealthChallengeStore, LinuxKernelEvidence, UpdateRuntimeError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeReconciliation {
    None,
    Ready,
    DiscardedStale,
    RolledBackMissing,
    RolledBackMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorActivation {
    pub outcome: ActivationOutcome,
    pub credential_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorRecovery {
    pub manager: RecoveryOutcome,
    pub challenge: ChallengeReconciliation,
    pub abandoned_staging_removed: usize,
    pub slot_deletions_completed: usize,
}

#[derive(Debug)]
pub struct UpdateSupervisor {
    manager: ApplicationUpdateManager,
    challenges: HealthChallengeStore,
    kernel: LinuxKernelEvidence,
}

impl UpdateSupervisor {
    pub fn new(manager: ApplicationUpdateManager, challenges: HealthChallengeStore) -> Self {
        Self {
            manager,
            challenges,
            kernel: LinuxKernelEvidence::new(),
        }
    }

    pub fn initialize(&self, active_slot_id: &str) -> Result<ManagerStatus, UpdateRuntimeError> {
        let _ = self.challenges.clear_stale_after_rollback()?;
        Ok(self.manager.initialize(active_slot_id)?)
    }

    pub fn status(&self) -> Result<ManagerStatus, UpdateRuntimeError> {
        Ok(self.manager.status()?)
    }

    pub fn activate(
        &self,
        target_slot_id: &str,
    ) -> Result<SupervisorActivation, UpdateRuntimeError> {
        let _ = self.watchdog_once()?;
        if self.manager.status()?.pending.is_some() {
            return Err(UpdateManagerError::PendingActivation.into());
        }
        let clock = self.kernel.snapshot()?;
        let deadline = clock
            .monotonic_ms
            .checked_add(self.manager.health_timeout_ms())
            .ok_or(UpdateRuntimeError::InvalidBootClock)?;
        let issued = self.challenges.issue(target_slot_id, clock, deadline)?;
        let request = ActivationRequest {
            operation_id: issued.operation_id,
            target_slot_id: target_slot_id.to_string(),
            health_binding: issued.health_binding,
            clock,
        };
        let outcome = match self.manager.activate(request) {
            Ok(outcome) => outcome,
            Err(error) => {
                self.challenges.discard(issued.operation_id)?;
                return Err(error.into());
            }
        };
        match &outcome {
            ActivationOutcome::AwaitingHealth {
                operation_id,
                slot_id,
                deadline_monotonic_ms,
            } if *operation_id == issued.operation_id
                && slot_id == target_slot_id
                && *deadline_monotonic_ms == deadline =>
            {
                Ok(SupervisorActivation {
                    outcome,
                    credential_path: Some(issued.credential_path),
                })
            }
            ActivationOutcome::AlreadyActive
            | ActivationOutcome::ReplayConfirmed
            | ActivationOutcome::ReplayRolledBack { .. } => {
                self.challenges.discard(issued.operation_id)?;
                Ok(SupervisorActivation {
                    outcome,
                    credential_path: None,
                })
            }
            ActivationOutcome::AwaitingHealth { .. } => {
                let _ = self.manager.abort_missing_health_challenge();
                let _ = self.challenges.discard(issued.operation_id);
                Err(UpdateRuntimeError::SupervisorStateMismatch)
            }
        }
    }

    pub fn confirm_from_reader(
        &self,
        reader: &mut impl Read,
    ) -> Result<ConfirmationOutcome, UpdateRuntimeError> {
        let status = self
            .challenges
            .status()?
            .ok_or(UpdateRuntimeError::ChallengeMissing)?;
        let mut response = [0_u8; 32];
        if reader.read_exact(&mut response).is_err() {
            response.zeroize();
            return Err(UpdateRuntimeError::InvalidHealthResponseLength);
        }
        let mut extra = [0_u8; 1];
        match reader.read(&mut extra) {
            Ok(0) => {}
            Ok(_) | Err(_) => {
                response.zeroize();
                return Err(UpdateRuntimeError::InvalidHealthResponseLength);
            }
        }
        let clock = match self.kernel.snapshot() {
            Ok(clock) => clock,
            Err(error) => {
                response.zeroize();
                return Err(error);
            }
        };
        let verified =
            self.challenges
                .verify_response(status.operation_id, &mut response, clock)?;
        let outcome =
            self.manager
                .confirm_health(verified.operation_id, verified.health_binding, clock)?;
        self.challenges.consume(verified.operation_id)?;
        Ok(outcome)
    }

    pub fn watchdog_once(&self) -> Result<SupervisorRecovery, UpdateRuntimeError> {
        let storage = self.manager.recover_update_storage()?;
        let abandoned_staging_removed = storage.abandoned_staging_removed;
        let slot_deletions_completed = storage.slot_deletions_completed;
        let clock = self.kernel.snapshot()?;
        let initial = self.manager.recover(clock)?;
        let status = self.manager.status()?;
        let challenge = match (status.pending.as_ref(), self.challenges.status()) {
            (Some(pending), Ok(Some(challenge))) if challenge_matches(pending, &challenge) => {
                ChallengeReconciliation::Ready
            }
            (Some(_), Ok(None)) => {
                let manager = self.manager.abort_missing_health_challenge()?;
                return Ok(SupervisorRecovery {
                    manager,
                    challenge: ChallengeReconciliation::RolledBackMissing,
                    abandoned_staging_removed,
                    slot_deletions_completed,
                });
            }
            (Some(_), Ok(Some(challenge))) => {
                let manager = self.manager.abort_missing_health_challenge()?;
                self.challenges.discard(challenge.operation_id)?;
                return Ok(SupervisorRecovery {
                    manager,
                    challenge: ChallengeReconciliation::RolledBackMismatch,
                    abandoned_staging_removed,
                    slot_deletions_completed,
                });
            }
            (Some(_), Err(UpdateRuntimeError::InvalidChallengeState)) => {
                let manager = self.manager.abort_missing_health_challenge()?;
                let _ = self.challenges.clear_stale_after_rollback()?;
                return Ok(SupervisorRecovery {
                    manager,
                    challenge: ChallengeReconciliation::RolledBackMismatch,
                    abandoned_staging_removed,
                    slot_deletions_completed,
                });
            }
            (Some(_), Err(error)) => return Err(error),
            (None, Ok(Some(challenge))) => {
                self.challenges.discard(challenge.operation_id)?;
                ChallengeReconciliation::DiscardedStale
            }
            (None, Err(UpdateRuntimeError::InvalidChallengeState)) => {
                self.challenges.clear_stale_after_rollback()?;
                ChallengeReconciliation::DiscardedStale
            }
            (None, Ok(None)) => ChallengeReconciliation::None,
            (None, Err(error)) => return Err(error),
        };
        Ok(SupervisorRecovery {
            manager: initial,
            challenge,
            abandoned_staging_removed,
            slot_deletions_completed,
        })
    }
}

fn challenge_matches(
    pending: &bli_kvm_update_manager::PendingStatus,
    challenge: &HealthChallengeStatus,
) -> bool {
    pending.operation_id == encode_hex(&challenge.operation_id)
        && pending.target_slot_id == challenge.target_slot_id
        && pending.boot_id == encode_hex(&challenge.boot_id)
        && pending.deadline_monotonic_ms == challenge.deadline_monotonic_ms
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    value
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs::{self, Permissions},
        io::Cursor,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    use bli_kvm_update_manager::{
        ApplicationUpdateManager, ConfirmationOutcome, DEFAULT_HEALTH_TIMEOUT_MS, RollbackReason,
    };
    use bli_kvm_update_stager::{ApplicationSlotStager, StagedApplication};
    use bli_kvm_update_verifier::{
        APPLICATION_ARCHITECTURE, APPLICATION_FORMAT_V1, APPLICATION_MANIFEST_PATH,
        APPLICATION_PRODUCT, APPLICATION_SIGNATURE_PATH, APPLICATION_TARGET, ApplicationFile,
        ApplicationManifest, ApplicationTrustRoot,
    };
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};
    use tar::{Builder, EntryType, Header};
    use tempfile::TempDir;

    use super::*;

    const KEY_ID: &str = "app-supervisor-test-key-1";

    struct Fixture {
        slots: TempDir,
        state: TempDir,
        runtime: TempDir,
        sources: TempDir,
        signing: SigningKey,
        trust: ApplicationTrustRoot,
    }

    impl Fixture {
        fn new() -> Self {
            let signing = SigningKey::from_bytes(&[37; 32]);
            let trust =
                ApplicationTrustRoot::new(KEY_ID, signing.verifying_key().to_bytes()).unwrap();
            Self {
                slots: tempfile::tempdir().unwrap(),
                state: tempfile::tempdir().unwrap(),
                runtime: tempfile::tempdir().unwrap(),
                sources: tempfile::tempdir().unwrap(),
                signing,
                trust,
            }
        }

        fn stage(&self, sequence: u64) -> StagedApplication {
            let payload = payload(sequence);
            let manifest = manifest(sequence, &payload);
            let archive = bundle(&self.signing, &manifest, &payload);
            let source = self
                .sources
                .path()
                .join(format!("application-{sequence}.tar"));
            fs::write(&source, archive).unwrap();
            fs::set_permissions(&source, Permissions::from_mode(0o600)).unwrap();
            ApplicationSlotStager::open(self.slots.path())
                .unwrap()
                .stage(source, &self.trust)
                .unwrap()
        }

        fn supervisor(&self) -> UpdateSupervisor {
            let manager = ApplicationUpdateManager::open(
                self.slots.path(),
                self.state.path(),
                self.trust.clone(),
                1,
                DEFAULT_HEALTH_TIMEOUT_MS,
            )
            .unwrap();
            let challenges = HealthChallengeStore::open(self.runtime.path()).unwrap();
            UpdateSupervisor::new(manager, challenges)
        }
    }

    #[test]
    fn activation_credential_confirms_exactly_once_without_argv_or_environment() {
        let fixture = Fixture::new();
        let slot1 = fixture.stage(1);
        let slot2 = fixture.stage(2);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&slot1.slot_id).unwrap();

        let activation = supervisor.activate(&slot2.slot_id).unwrap();
        assert!(matches!(
            activation.outcome,
            ActivationOutcome::AwaitingHealth { .. }
        ));
        let credential = activation.credential_path.unwrap();
        let token = fs::read(&credential).unwrap();
        assert_eq!(token.len(), 32);

        let mut oversized = token.clone();
        oversized.push(0);
        assert!(matches!(
            supervisor.confirm_from_reader(&mut Cursor::new(oversized)),
            Err(UpdateRuntimeError::InvalidHealthResponseLength)
        ));
        assert!(credential.exists());

        assert_eq!(
            supervisor
                .confirm_from_reader(&mut Cursor::new(token))
                .unwrap(),
            ConfirmationOutcome::Confirmed
        );
        assert!(!credential.exists());
        let status = supervisor.status().unwrap();
        assert_eq!(status.active_slot_id, slot2.slot_id);
        assert_eq!(status.floor_sequence, 2);
    }

    #[test]
    fn missing_credential_forces_manager_rollback_and_exact_cleanup() {
        let fixture = Fixture::new();
        let slot1 = fixture.stage(1);
        let slot2 = fixture.stage(2);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&slot1.slot_id).unwrap();
        let activation = supervisor.activate(&slot2.slot_id).unwrap();
        fs::remove_file(activation.credential_path.unwrap()).unwrap();

        let recovery = supervisor.watchdog_once().unwrap();
        assert!(matches!(
            recovery.manager,
            RecoveryOutcome::RolledBack {
                reason: RollbackReason::HealthChallengeLost,
                ..
            }
        ));
        assert_eq!(
            recovery.challenge,
            ChallengeReconciliation::RolledBackMismatch
        );
        assert_eq!(supervisor.status().unwrap().active_slot_id, slot1.slot_id);
        assert_eq!(fs::read_dir(fixture.runtime.path()).unwrap().count(), 0);
    }

    #[test]
    fn watchdog_discards_a_complete_challenge_without_pending_manager_state() {
        let fixture = Fixture::new();
        let slot1 = fixture.stage(1);
        let slot2 = fixture.stage(2);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&slot1.slot_id).unwrap();
        let clock = LinuxKernelEvidence::new().snapshot().unwrap();
        let issued = supervisor
            .challenges
            .issue(
                &slot2.slot_id,
                clock,
                clock.monotonic_ms + DEFAULT_HEALTH_TIMEOUT_MS,
            )
            .unwrap();

        let recovery = supervisor.watchdog_once().unwrap();
        assert_eq!(recovery.manager, RecoveryOutcome::Clean);
        assert_eq!(recovery.challenge, ChallengeReconciliation::DiscardedStale);
        assert!(!issued.credential_path.exists());
        assert_eq!(supervisor.status().unwrap().active_slot_id, slot1.slot_id);
    }

    #[test]
    fn activating_the_current_slot_is_a_credential_free_noop() {
        let fixture = Fixture::new();
        let slot1 = fixture.stage(1);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&slot1.slot_id).unwrap();
        let activation = supervisor.activate(&slot1.slot_id).unwrap();
        assert_eq!(activation.outcome, ActivationOutcome::AlreadyActive);
        assert_eq!(activation.credential_path, None);
        assert_eq!(fs::read_dir(fixture.runtime.path()).unwrap().count(), 0);
    }

    fn payload(sequence: u64) -> BTreeMap<&'static str, (u32, Vec<u8>)> {
        BTreeMap::from([
            (
                "bin/bli-kvm-server",
                (0o755, format!("server-{sequence}").into_bytes()),
            ),
            (
                "libexec/bli-kvm-control-broker",
                (0o755, format!("control-{sequence}").into_bytes()),
            ),
            (
                "libexec/bli-kvm-platform-broker",
                (0o755, format!("broker-{sequence}").into_bytes()),
            ),
            (
                "libexec/bli-kvm-video-proxy",
                (0o755, format!("video-{sequence}").into_bytes()),
            ),
            (
                "share/web/index.html",
                (0o644, format!("<html>{sequence}</html>").into_bytes()),
            ),
        ])
    }

    fn manifest(sequence: u64, payload: &BTreeMap<&str, (u32, Vec<u8>)>) -> ApplicationManifest {
        ApplicationManifest {
            format: APPLICATION_FORMAT_V1.into(),
            product: APPLICATION_PRODUCT.into(),
            target: APPLICATION_TARGET.into(),
            architecture: APPLICATION_ARCHITECTURE.into(),
            key_id: KEY_ID.into(),
            version: format!("0.{sequence}.0"),
            sequence,
            minimum_system_sequence: 1,
            source_commit: format!("{sequence:040x}"),
            files: payload
                .iter()
                .map(|(path, (mode, bytes))| ApplicationFile {
                    path: (*path).to_string(),
                    mode: *mode,
                    size: bytes.len() as u64,
                    sha256: sha256_hex(bytes),
                })
                .collect(),
        }
    }

    fn header(path: &str, mode: u32, size: u64, entry_type: EntryType) -> Header {
        let mut header = Header::new_ustar();
        header.set_path(path).unwrap();
        header.set_mode(mode);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_size(size);
        header.set_entry_type(entry_type);
        header.set_cksum();
        header
    }

    fn bundle(
        signing_key: &SigningKey,
        manifest: &ApplicationManifest,
        payload: &BTreeMap<&str, (u32, Vec<u8>)>,
    ) -> Vec<u8> {
        let manifest_bytes = serde_json::to_vec(manifest).unwrap();
        let signature = signing_key.sign(&manifest_bytes).to_bytes();
        let mut builder = Builder::new(Vec::new());
        builder.mode(tar::HeaderMode::Deterministic);
        builder
            .append(
                &header(
                    APPLICATION_MANIFEST_PATH,
                    0o644,
                    manifest_bytes.len() as u64,
                    EntryType::Regular,
                ),
                manifest_bytes.as_slice(),
            )
            .unwrap();
        builder
            .append(
                &header(APPLICATION_SIGNATURE_PATH, 0o644, 64, EntryType::Regular),
                signature.as_slice(),
            )
            .unwrap();
        let directories = payload_directories(payload);
        let mut entries = directories.clone();
        entries.extend(payload.keys().map(|path| format!("payload/{path}")));
        for path in entries {
            if directories.contains(&path) {
                builder
                    .append(&header(&path, 0o755, 0, EntryType::Directory), &[][..])
                    .unwrap();
            } else {
                let relative = path.strip_prefix("payload/").unwrap();
                let (mode, bytes) = payload.get(relative).unwrap();
                builder
                    .append(
                        &header(&path, *mode, bytes.len() as u64, EntryType::Regular),
                        bytes.as_slice(),
                    )
                    .unwrap();
            }
        }
        builder.into_inner().unwrap()
    }

    fn payload_directories(payload: &BTreeMap<&str, (u32, Vec<u8>)>) -> BTreeSet<String> {
        let mut directories = BTreeSet::from(["payload".to_string()]);
        for path in payload.keys() {
            let mut current = PathBuf::from("payload");
            let components = Path::new(path).components().collect::<Vec<_>>();
            for component in components.iter().take(components.len() - 1) {
                current.push(component.as_os_str());
                directories.insert(current.to_str().unwrap().to_string());
            }
        }
        directories
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let mut value = String::with_capacity(digest.len() * 2);
        for byte in digest {
            use std::fmt::Write as _;
            write!(&mut value, "{byte:02x}").unwrap();
        }
        value
    }
}
