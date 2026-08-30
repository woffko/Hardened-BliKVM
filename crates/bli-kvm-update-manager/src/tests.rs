use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, Permissions},
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
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

const KEY_ID: &str = "app-manager-test-key-1";

struct Fixture {
    slots: TempDir,
    state: TempDir,
    sources: TempDir,
    signing: SigningKey,
    trust: ApplicationTrustRoot,
}

impl Fixture {
    fn new() -> Self {
        let signing = SigningKey::from_bytes(&[27; 32]);
        let trust = ApplicationTrustRoot::new(KEY_ID, signing.verifying_key().to_bytes()).unwrap();
        Self {
            slots: tempfile::tempdir().unwrap(),
            state: tempfile::tempdir().unwrap(),
            sources: tempfile::tempdir().unwrap(),
            signing,
            trust,
        }
    }

    fn stage(&self, sequence: u64, variant: u8, minimum_system: u64) -> StagedApplication {
        let payload = payload(sequence, variant);
        let manifest = manifest(sequence, variant, minimum_system, &payload);
        let archive = bundle(&self.signing, &manifest, &payload);
        let source = self
            .sources
            .path()
            .join(format!("application-{sequence}-{variant}.tar"));
        fs::write(&source, archive).unwrap();
        fs::set_permissions(&source, Permissions::from_mode(0o600)).unwrap();
        ApplicationSlotStager::open(self.slots.path())
            .unwrap()
            .stage(source, &self.trust)
            .unwrap()
    }

    fn manager(&self, system_sequence: u64) -> ApplicationUpdateManager {
        ApplicationUpdateManager::open(
            self.slots.path(),
            self.state.path(),
            self.trust.clone(),
            system_sequence,
            DEFAULT_HEALTH_TIMEOUT_MS,
        )
        .unwrap()
    }

    fn current(&self) -> String {
        fs::read_link(self.slots.path().join("current"))
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    }
}

fn clock(boot: u8, monotonic_ms: u64) -> ClockSnapshot {
    ClockSnapshot {
        boot_id: [boot; 16],
        monotonic_ms,
    }
}

fn request(operation: u8, target: &StagedApplication, boot: u8, now: u64) -> ActivationRequest {
    ActivationRequest {
        operation_id: [operation; 16],
        target_slot_id: target.slot_id.clone(),
        health_binding: [operation.wrapping_add(64); 32],
        clock: clock(boot, now),
    }
}

#[test]
fn initializes_confirms_and_enforces_the_persisted_floor() {
    let fixture = Fixture::new();
    let slot1 = fixture.stage(1, 1, 1);
    let slot2 = fixture.stage(2, 1, 1);
    let alternate2 = fixture.stage(2, 2, 1);
    let manager = fixture.manager(1);
    let initialized = manager.initialize(&slot1.slot_id).unwrap();
    assert_eq!(initialized.floor_sequence, 1);
    assert_eq!(fixture.current(), slot1.slot_id);

    let activation = request(1, &slot2, 1, 1_000);
    assert_eq!(
        manager.activate(activation.clone()).unwrap(),
        ActivationOutcome::AwaitingHealth {
            operation_id: [1; 16],
            slot_id: slot2.slot_id.clone(),
            deadline_monotonic_ms: 61_000,
        }
    );
    let pending = manager.status().unwrap();
    assert_eq!(pending.active_slot_id, slot2.slot_id);
    assert_eq!(pending.confirmed_slot_id, slot1.slot_id);
    assert_eq!(pending.floor_sequence, 1);
    assert_eq!(fixture.current(), slot2.slot_id);
    assert!(matches!(
        manager.confirm_health([1; 16], [99; 32], clock(1, 2_000)),
        Err(UpdateManagerError::OperationConflict)
    ));
    assert_eq!(
        manager
            .confirm_health([1; 16], [65; 32], clock(1, 2_000))
            .unwrap(),
        ConfirmationOutcome::Confirmed
    );
    assert_eq!(
        manager
            .confirm_health([1; 16], [65; 32], clock(1, 2_001))
            .unwrap(),
        ConfirmationOutcome::ReplayConfirmed
    );
    assert_eq!(
        manager.activate(activation).unwrap(),
        ActivationOutcome::ReplayConfirmed
    );
    let confirmed = manager.status().unwrap();
    assert_eq!(confirmed.floor_sequence, 2);
    assert_eq!(confirmed.confirmed_slot_id, slot2.slot_id);

    assert!(matches!(
        manager.activate(request(2, &slot1, 1, 3_000)),
        Err(UpdateManagerError::AntiRollback)
    ));
    assert!(matches!(
        manager.activate(request(3, &alternate2, 1, 3_000)),
        Err(UpdateManagerError::AntiRollback)
    ));
}

#[test]
fn timeout_reboot_and_explicit_failure_all_rollback_without_raising_the_floor() {
    let fixture = Fixture::new();
    let slot1 = fixture.stage(1, 1, 1);
    let slot2 = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&slot1.slot_id).unwrap();

    let first = request(10, &slot2, 2, 100);
    manager.activate(first.clone()).unwrap();
    let mut wrong_boot_retry = first.clone();
    wrong_boot_retry.clock = clock(3, 1);
    assert!(matches!(
        manager.activate(wrong_boot_retry),
        Err(UpdateManagerError::RecoveryRequired)
    ));
    assert!(matches!(
        manager.recover(clock(2, 60_100)).unwrap(),
        RecoveryOutcome::AwaitingHealth { .. }
    ));
    assert_eq!(
        manager.recover(clock(2, 60_101)).unwrap(),
        RecoveryOutcome::RolledBack {
            active_slot_id: slot1.slot_id.clone(),
            rejected_slot_id: slot2.slot_id.clone(),
            reason: RollbackReason::HealthTimeout,
        }
    );
    assert_eq!(fixture.current(), slot1.slot_id);
    assert_eq!(manager.status().unwrap().floor_sequence, 1);
    assert_eq!(
        manager.activate(first).unwrap(),
        ActivationOutcome::ReplayRolledBack {
            reason: RollbackReason::HealthTimeout,
        }
    );

    let second = request(11, &slot2, 2, 70_000);
    manager.activate(second).unwrap();
    assert!(matches!(
        manager.recover(clock(3, 1)).unwrap(),
        RecoveryOutcome::RolledBack {
            reason: RollbackReason::BootChanged,
            ..
        }
    ));

    let third = request(12, &slot2, 3, 2_000);
    manager.activate(third).unwrap();
    assert!(matches!(
        manager.reject_health([12; 16], [76; 32]).unwrap(),
        RecoveryOutcome::RolledBack {
            reason: RollbackReason::ExplicitHealthFailure,
            ..
        }
    ));
    assert!(matches!(
        manager.reject_health([12; 16], [76; 32]).unwrap(),
        RecoveryOutcome::RolledBack {
            reason: RollbackReason::ExplicitHealthFailure,
            ..
        }
    ));
    assert_eq!(manager.status().unwrap().floor_sequence, 1);
}

#[test]
fn recovery_covers_power_loss_before_and_after_the_pointer_switch() {
    let fixture = Fixture::new();
    let slot1 = fixture.stage(1, 1, 1);
    let slot2 = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&slot1.slot_id).unwrap();

    let before_switch = request(20, &slot2, 4, 100);
    assert!(matches!(
        manager.prepare_activation(&before_switch).unwrap(),
        PrepareResult::Prepared
    ));
    drop(manager);
    let manager = fixture.manager(1);
    assert!(matches!(
        manager.recover(clock(4, 101)).unwrap(),
        RecoveryOutcome::RolledBack {
            reason: RollbackReason::InterruptedPrepare,
            ..
        }
    ));
    assert_eq!(fixture.current(), slot1.slot_id);

    let after_switch = request(21, &slot2, 4, 200);
    assert!(matches!(
        manager.prepare_activation(&after_switch).unwrap(),
        PrepareResult::Prepared
    ));
    manager.pointer.switch(&slot2.slot_id).unwrap();
    drop(manager);
    let manager = fixture.manager(1);
    assert!(matches!(
        manager.recover(clock(4, 201)).unwrap(),
        RecoveryOutcome::RolledBack {
            reason: RollbackReason::InterruptedPrepare,
            ..
        }
    ));
    assert_eq!(fixture.current(), slot1.slot_id);
}

#[test]
fn recovery_finishes_an_interrupted_rollback_and_repairs_an_invalid_symlink_target() {
    let fixture = Fixture::new();
    let slot1 = fixture.stage(1, 1, 1);
    let slot2 = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&slot1.slot_id).unwrap();
    manager.activate(request(30, &slot2, 5, 100)).unwrap();
    manager.pointer.switch(&slot1.slot_id).unwrap();
    drop(manager);

    let manager = fixture.manager(1);
    assert!(matches!(
        manager.recover(clock(5, 101)).unwrap(),
        RecoveryOutcome::RolledBack {
            reason: RollbackReason::PointerMismatch,
            ..
        }
    ));
    let current_path = fixture.slots.path().join("current");
    fs::remove_file(&current_path).unwrap();
    symlink("../escape", &current_path).unwrap();
    drop(manager);

    let manager = fixture.manager(1);
    assert_eq!(
        manager.recover(clock(5, 102)).unwrap(),
        RecoveryOutcome::RepairedPointer {
            slot_id: slot1.slot_id.clone(),
        }
    );
    assert_eq!(fixture.current(), slot1.slot_id);
}

#[test]
fn a_corrupt_candidate_cannot_block_rollback_to_the_verified_anchor() {
    let fixture = Fixture::new();
    let slot1 = fixture.stage(1, 1, 1);
    let slot2 = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&slot1.slot_id).unwrap();
    manager.activate(request(35, &slot2, 5, 500)).unwrap();

    let candidate_server = slot2.slot_path.join("bin/bli-kvm-server");
    fs::set_permissions(&candidate_server, Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        manager.recover(clock(5, 501)).unwrap(),
        RecoveryOutcome::RolledBack {
            active_slot_id: slot1.slot_id.clone(),
            rejected_slot_id: slot2.slot_id,
            reason: RollbackReason::TargetInvalid,
        }
    );
    let status = manager.status().unwrap();
    assert_eq!(status.active_slot_id, slot1.slot_id);
    assert_eq!(status.previous_slot_id, None);
    assert_eq!(fixture.current(), slot1.slot_id);
}

#[test]
fn recovery_drops_a_corrupt_non_authoritative_previous_slot() {
    let fixture = Fixture::new();
    let slot1 = fixture.stage(1, 1, 1);
    let slot2 = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&slot1.slot_id).unwrap();
    manager.activate(request(36, &slot2, 5, 600)).unwrap();
    manager.reject_health([36; 16], [100; 32]).unwrap();
    fs::set_permissions(
        slot2.slot_path.join("bin/bli-kvm-server"),
        Permissions::from_mode(0o600),
    )
    .unwrap();
    assert_eq!(
        manager.recover(clock(5, 601)).unwrap(),
        RecoveryOutcome::DroppedInvalidPrevious {
            slot_id: slot1.slot_id.clone(),
        }
    );
    assert_eq!(manager.status().unwrap().previous_slot_id, None);
}

#[test]
fn rejects_incompatible_slots_invalid_operations_and_concurrent_managers() {
    let fixture = Fixture::new();
    let slot1 = fixture.stage(1, 1, 1);
    let incompatible = fixture.stage(2, 1, 2);
    let manager = fixture.manager(1);
    manager.initialize(&slot1.slot_id).unwrap();
    assert!(matches!(
        manager.activate(request(40, &incompatible, 6, 100)),
        Err(UpdateManagerError::MinimumSystemSequence)
    ));
    let mut invalid = request(41, &incompatible, 6, 100);
    invalid.operation_id = [0; 16];
    assert!(matches!(
        manager.activate(invalid),
        Err(UpdateManagerError::InvalidOperation)
    ));
    assert!(matches!(
        ApplicationUpdateManager::open(
            fixture.slots.path(),
            fixture.state.path(),
            fixture.trust.clone(),
            1,
            DEFAULT_HEALTH_TIMEOUT_MS,
        ),
        Err(UpdateManagerError::ManagerBusy)
    ));
}

#[test]
fn unsafe_journal_permissions_are_never_repaired_implicitly() {
    let fixture = Fixture::new();
    let slot1 = fixture.stage(1, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&slot1.slot_id).unwrap();
    let journal = fixture.state.path().join(journal::JOURNAL_FILE_NAME);
    fs::set_permissions(&journal, Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        manager.status(),
        Err(UpdateManagerError::UnsafeJournal)
    ));
}

#[test]
fn operation_history_exhaustion_is_explicit_and_never_prunes_replay_evidence() {
    let fixture = Fixture::new();
    let slot1 = fixture.stage(1, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&slot1.slot_id).unwrap();
    for operation in 1..=128_u8 {
        assert_eq!(
            manager
                .activate(request(operation, &slot1, 7, u64::from(operation)))
                .unwrap(),
            ActivationOutcome::AlreadyActive
        );
    }
    assert!(matches!(
        manager.activate(request(129, &slot1, 7, 129)),
        Err(UpdateManagerError::HistoryFull)
    ));
    assert_eq!(manager.status().unwrap().active_slot_id, slot1.slot_id);
}

#[test]
fn garbage_collection_protects_journal_references_and_retains_newest_candidates() {
    let fixture = Fixture::new();
    let slot1 = fixture.stage(1, 1, 1);
    let slot2 = fixture.stage(2, 1, 1);
    let slot3 = fixture.stage(3, 1, 1);
    let slot4 = fixture.stage(4, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&slot1.slot_id).unwrap();
    manager.activate(request(70, &slot2, 8, 100)).unwrap();
    manager
        .confirm_health([70; 16], [134; 32], clock(8, 101))
        .unwrap();

    assert!(matches!(
        manager.garbage_collect(MAX_UNREFERENCED_SLOT_RETENTION + 1),
        Err(UpdateManagerError::InvalidRetention)
    ));
    let collected = manager.garbage_collect(1).unwrap();
    assert_eq!(collected.deleted_slot_ids, vec![slot3.slot_id.clone()]);
    assert_eq!(collected.retained_unreferenced, 1);
    assert_eq!(manager.status().unwrap().active_slot_id, slot2.slot_id);
    drop(manager);

    let stager = ApplicationSlotStager::open(fixture.slots.path()).unwrap();
    let remaining = stager
        .list_slots(&fixture.trust)
        .unwrap()
        .into_iter()
        .map(|slot| slot.slot_id)
        .collect::<Vec<_>>();
    assert_eq!(remaining, vec![slot1.slot_id, slot2.slot_id, slot4.slot_id]);
}

fn payload(sequence: u64, variant: u8) -> BTreeMap<&'static str, (u32, Vec<u8>)> {
    BTreeMap::from([
        (
            "bin/bli-kvm-server",
            (0o755, format!("server-{sequence}-{variant}").into_bytes()),
        ),
        (
            "libexec/bli-kvm-control-broker",
            (0o755, format!("control-{sequence}-{variant}").into_bytes()),
        ),
        (
            "libexec/bli-kvm-platform-broker",
            (0o755, format!("broker-{sequence}-{variant}").into_bytes()),
        ),
        (
            "libexec/bli-kvm-video-proxy",
            (0o755, format!("video-{sequence}-{variant}").into_bytes()),
        ),
        (
            "share/web/index.html",
            (
                0o644,
                format!("<html>{sequence}-{variant}</html>").into_bytes(),
            ),
        ),
    ])
}

fn manifest(
    sequence: u64,
    variant: u8,
    minimum_system: u64,
    payload: &BTreeMap<&str, (u32, Vec<u8>)>,
) -> ApplicationManifest {
    ApplicationManifest {
        format: APPLICATION_FORMAT_V1.into(),
        product: APPLICATION_PRODUCT.into(),
        target: APPLICATION_TARGET.into(),
        architecture: APPLICATION_ARCHITECTURE.into(),
        key_id: KEY_ID.into(),
        version: format!("0.{sequence}.{variant}"),
        sequence,
        minimum_system_sequence: minimum_system,
        source_commit: format!("{:040x}", sequence * 256 + u64::from(variant)),
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
