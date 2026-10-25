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
fn native_confirmation_requires_readiness_and_allows_a_subsequent_update() {
    let fixture = Fixture::new();
    let confirmed = fixture.stage(1, 1, 1);
    let candidate = fixture.stage(2, 1, 1);
    let next = fixture.stage(3, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&confirmed.slot_id).unwrap();
    let idle = manager.migrate_native_idle(1, |_| Ok(())).unwrap();
    let activation = request(1, &candidate, 1, 100);
    manager.admit_native_activation(idle, &activation).unwrap();
    let prepared = manager.inspect_recovery().unwrap();
    manager
        .execute_native_prepared_activation(
            prepared.generation,
            activation.clone(),
            || Ok(()),
            || Ok(clock(1, 101)),
        )
        .unwrap();
    let before = manager.state.load().unwrap().unwrap();
    assert!(
        manager
            .confirm_native_activation(
                before.generation - 1,
                &activation,
                || panic!("stale"),
                || panic!("stale")
            )
            .is_err()
    );
    let mut wrong = activation.clone();
    wrong.health_binding = [99; 32];
    assert!(
        manager
            .confirm_native_activation(
                before.generation,
                &wrong,
                || panic!("wrong binding"),
                || panic!("wrong binding")
            )
            .is_err()
    );
    assert_eq!(
        manager
            .confirm_native_activation(
                before.generation,
                &activation,
                || {
                    assert_eq!(manager.state.load().unwrap().unwrap(), before);
                    assert_eq!(fixture.current(), candidate.slot_id);
                    Ok(())
                },
                || Ok(clock(1, 100 + DEFAULT_HEALTH_TIMEOUT_MS))
            )
            .unwrap(),
        NativeConfirmationOutcome::Confirmed
    );
    let complete = manager.state.load().unwrap().unwrap();
    assert_eq!(
        complete.schema_version,
        model::NATIVE_LIFECYCLE_SCHEMA_VERSION
    );
    assert_eq!(complete.floor_sequence, 2);
    assert!(complete.pending.is_none());
    assert!(complete.native_rollback.is_none());
    assert_eq!(
        manager
            .confirm_native_activation(
                complete.generation,
                &activation,
                || panic!("replay readiness"),
                || panic!("replay clock")
            )
            .unwrap(),
        NativeConfirmationOutcome::ReplayConfirmed
    );
    assert_eq!(manager.state.load().unwrap().unwrap(), complete);
    assert_eq!(
        manager
            .admit_native_activation(complete.generation, &request(2, &next, 1, 70_000))
            .unwrap(),
        ActivationPreparation::Prepared
    );
}

#[test]
fn native_confirmation_failure_persists_intent_without_legacy_rollback() {
    for reason in [
        RollbackReason::ExplicitHealthFailure,
        RollbackReason::HealthTimeout,
        RollbackReason::BootChanged,
        RollbackReason::TargetInvalid,
    ] {
        let fixture = Fixture::new();
        let confirmed = fixture.stage(1, 1, 1);
        let candidate = fixture.stage(2, 1, 1);
        let manager = fixture.manager(1);
        manager.initialize(&confirmed.slot_id).unwrap();
        let idle = manager.migrate_native_idle(1, |_| Ok(())).unwrap();
        let activation = request(1, &candidate, 1, 100);
        manager.admit_native_activation(idle, &activation).unwrap();
        let prepared = manager.inspect_recovery().unwrap();
        manager
            .execute_native_prepared_activation(
                prepared.generation,
                activation.clone(),
                || Ok(()),
                || Ok(clock(1, 101)),
            )
            .unwrap();
        let before = manager.state.load().unwrap().unwrap();
        let result = manager
            .confirm_native_activation(
                before.generation,
                &activation,
                || {
                    if reason == RollbackReason::TargetInvalid {
                        let executable = fixture
                            .slots
                            .path()
                            .join(&candidate.slot_id)
                            .join("bin/bli-kvm-server");
                        fs::set_permissions(&executable, Permissions::from_mode(0o600)).unwrap();
                        fs::write(executable, b"damaged candidate").unwrap();
                    }
                    if reason == RollbackReason::ExplicitHealthFailure {
                        Err(UpdateManagerError::RecoveryRequired)
                    } else {
                        Ok(())
                    }
                },
                || {
                    Ok(match reason {
                        RollbackReason::HealthTimeout => clock(1, 101 + DEFAULT_HEALTH_TIMEOUT_MS),
                        RollbackReason::BootChanged => clock(2, 1),
                        _ => clock(1, 102),
                    })
                },
            )
            .unwrap();
        let NativeConfirmationOutcome::RecoveryPending { intent } = result else {
            panic!("failure must retain recovery responsibility");
        };
        assert_eq!(intent.reason, reason);
        assert_eq!(intent.phase, NativeRollbackPhase::RollbackPending);
        let rejected = manager.state.load().unwrap().unwrap();
        assert_eq!(
            rejected.schema_version,
            model::NATIVE_LIFECYCLE_SCHEMA_VERSION
        );
        assert_eq!(rejected.floor_sequence, before.floor_sequence);
        assert_eq!(rejected.active, before.active);
        assert_eq!(rejected.pending, before.pending);
        assert_eq!(rejected.history, before.history);
        assert_eq!(fixture.current(), candidate.slot_id);
        assert!(
            manager
                .confirm_health([1; 16], [65; 32], clock(1, 102))
                .is_err()
        );
    }
}

#[test]
fn native_confirmation_refuses_expiry_before_probe_and_changed_state_after_probe() {
    for case in 0..4 {
        let fixture = Fixture::new();
        let confirmed = fixture.stage(1, 1, 1);
        let candidate = fixture.stage(2, 1, 1);
        let manager = fixture.manager(1);
        manager.initialize(&confirmed.slot_id).unwrap();
        let idle = manager.migrate_native_idle(1, |_| Ok(())).unwrap();
        let mut activation = request(1, &candidate, 1, 100);
        manager.admit_native_activation(idle, &activation).unwrap();
        let prepared = manager.inspect_recovery().unwrap();
        manager
            .execute_native_prepared_activation(
                prepared.generation,
                activation.clone(),
                || Ok(()),
                || Ok(clock(1, 101)),
            )
            .unwrap();
        let before = manager.state.load().unwrap().unwrap();
        if case == 0 {
            activation.clock = clock(1, 101 + DEFAULT_HEALTH_TIMEOUT_MS);
        }
        if case == 1 {
            activation.clock = clock(1, 99);
        }
        let result = manager.confirm_native_activation(
            before.generation,
            &activation,
            || {
                assert!(case >= 2, "invalid time must not invoke readiness");
                if case == 2 {
                    let mut changed = before.clone();
                    changed.last_rollback = Some(RollbackReason::HealthTimeout);
                    manager.state.store(&changed, Some(before.generation))?;
                }
                Ok(())
            },
            || Ok(clock(1, if case == 3 { 99 } else { 102 })),
        );
        if case == 0 {
            assert!(matches!(
                result,
                Ok(NativeConfirmationOutcome::RecoveryPending { .. })
            ));
        } else {
            assert!(result.is_err());
        }
        let after = manager.state.load().unwrap().unwrap();
        assert_eq!(after.floor_sequence, before.floor_sequence);
        assert_eq!(after.pending, before.pending);
        assert_eq!(fixture.current(), candidate.slot_id);
        if case == 1 || case == 3 {
            assert_eq!(after, before);
        }
    }
}

#[test]
fn native_admission_executes_after_reopen_and_supports_next_operation_after_recovery() {
    let fixture = Fixture::new();
    let confirmed = fixture.stage(1, 1, 1);
    let candidate = fixture.stage(2, 1, 1);
    let next = fixture.stage(3, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&confirmed.slot_id).unwrap();
    let request = request(1, &candidate, 1, 100);
    assert!(manager.admit_native_activation(1, &request).is_err());
    let idle = manager.migrate_native_idle(1, |_| Ok(())).unwrap();
    assert!(manager.admit_native_activation(idle - 1, &request).is_err());
    assert_eq!(
        manager.admit_native_activation(idle, &request).unwrap(),
        ActivationPreparation::Prepared
    );
    assert_eq!(fixture.current(), confirmed.slot_id);
    let prepared = manager.inspect_recovery().unwrap();
    assert!(
        manager
            .execute_native_prepared_activation(
                prepared.generation - 1,
                request.clone(),
                || panic!("stale execution must not quiesce"),
                || Ok(clock(1, 101))
            )
            .is_err()
    );
    assert!(
        manager
            .execute_prepared_activation(
                request.clone(),
                || panic!("legacy execution must not quiesce"),
                || Ok(clock(1, 101))
            )
            .is_err()
    );
    drop(manager);
    let manager = fixture.manager(1);
    assert!(matches!(
        manager
            .execute_native_prepared_activation(
                prepared.generation,
                request.clone(),
                || {
                    assert_eq!(fixture.current(), confirmed.slot_id);
                    Ok(())
                },
                || Ok(clock(1, 101))
            )
            .unwrap(),
        ActivationOutcome::AwaitingHealth { .. }
    ));
    let awaiting = manager.inspect_recovery().unwrap();
    assert!(matches!(
        manager
            .admit_native_activation(awaiting.generation, &request)
            .unwrap(),
        ActivationPreparation::Immediate(ActivationOutcome::AwaitingHealth { .. })
    ));
    let intent = manager
        .begin_native_rejection(awaiting.generation, [1; 16])
        .unwrap();
    let rejected = manager.inspect_recovery().unwrap();
    assert!(
        manager
            .admit_native_activation(rejected.generation, &request)
            .is_err()
    );
    // Status is not reported while a rollback intent is in progress.
    assert!(matches!(
        manager.observed_status(),
        Err(UpdateManagerError::RecoveryRequired)
    ));
    let restored = manager
        .restore_native_selection(rejected.generation, &intent, || Ok(()))
        .unwrap();
    manager
        .acknowledge_native_restoration(
            restored.generation,
            restored.rollback_intent.as_ref().unwrap(),
            || Ok(()),
        )
        .unwrap();
    let completed = manager.inspect_recovery().unwrap();
    // A completed, not yet archived restoration reports the confirmed slot.
    let observed = manager.observed_status().unwrap();
    assert_eq!(observed.generation, completed.generation);
    assert_eq!(observed.active_slot_id, completed.confirmed_slot_id);
    assert!(observed.pending.is_none());
    assert!(observed.last_rollback.is_some());
    assert!(manager.native_status().is_err());
    assert!(
        manager
            .admit_native_activation(completed.generation, &request)
            .is_err()
    );
    let idle = manager
        .migrate_native_idle(completed.generation, |_| Ok(()))
        .unwrap();
    assert!(matches!(
        manager.admit_native_activation(idle, &request).unwrap(),
        ActivationPreparation::Immediate(ActivationOutcome::ReplayRolledBack { .. })
    ));
    let next_request = ActivationRequest {
        operation_id: [2; 16],
        target_slot_id: next.slot_id,
        health_binding: [66; 32],
        clock: clock(1, 200),
    };
    assert_eq!(
        manager
            .admit_native_activation(idle, &next_request)
            .unwrap(),
        ActivationPreparation::Prepared
    );
    let document = manager.state.load().unwrap().unwrap();
    assert!(document.history[0].native_completion.is_some());
    assert_eq!(document.floor_sequence, confirmed.manifest.sequence);
}

#[test]
fn native_executor_retains_prepared_after_deadline_or_callback_state_change() {
    for changed_document in [false, true] {
        let fixture = Fixture::new();
        let confirmed = fixture.stage(1, 1, 1);
        let candidate = fixture.stage(2, 1, 1);
        let manager = fixture.manager(1);
        manager.initialize(&confirmed.slot_id).unwrap();
        let idle = manager.migrate_native_idle(1, |_| Ok(())).unwrap();
        let activation = request(1, &candidate, 1, 100);
        manager.admit_native_activation(idle, &activation).unwrap();
        let before = manager.state.load().unwrap().unwrap();
        assert!(
            manager
                .execute_native_prepared_activation(
                    before.generation,
                    activation,
                    || {
                        if changed_document {
                            let mut changed = before.clone();
                            changed.last_rollback = Some(RollbackReason::HealthTimeout);
                            manager.state.store(&changed, Some(before.generation))?;
                        }
                        Ok(())
                    },
                    || Ok(clock(
                        1,
                        if changed_document {
                            101
                        } else {
                            100 + DEFAULT_HEALTH_TIMEOUT_MS + 1
                        }
                    ))
                )
                .is_err()
        );
        assert_eq!(fixture.current(), confirmed.slot_id);
        assert_eq!(
            manager.inspect_recovery().unwrap().pending.unwrap().phase,
            PendingPhaseStatus::Prepared
        );
    }
}

#[test]
fn schema3_restoration_preserves_native_mode_and_bounds_archival_storage() {
    let fixture = Fixture::new();
    let confirmed = fixture.stage(1, 1, 1);
    let candidate = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&confirmed.slot_id).unwrap();
    manager.activate(request(72, &candidate, 8, 100)).unwrap();
    // Seed a valid native pending document; native admission is implemented separately.
    let mut pending = manager.state.load().unwrap().unwrap();
    pending.schema_version = model::NATIVE_LIFECYCLE_SCHEMA_VERSION;
    manager
        .state
        .store(&pending, Some(pending.generation))
        .unwrap();
    let intent = manager
        .begin_native_rejection(pending.generation, [72; 16])
        .unwrap();
    let rejected = manager.state.load().unwrap().unwrap();
    assert_eq!(
        rejected.schema_version,
        model::NATIVE_LIFECYCLE_SCHEMA_VERSION
    );
    let restored = manager
        .restore_native_selection(rejected.generation, &intent, || Ok(()))
        .unwrap();
    manager
        .acknowledge_native_restoration(
            restored.generation,
            restored.rollback_intent.as_ref().unwrap(),
            || Ok(()),
        )
        .unwrap();
    let completed = manager.state.load().unwrap().unwrap();
    assert_eq!(
        completed.schema_version,
        model::NATIVE_LIFECYCLE_SCHEMA_VERSION
    );
    manager
        .migrate_native_idle(completed.generation, |_| Ok(()))
        .unwrap();
    let archived = manager.state.load().unwrap().unwrap();
    let mut too_large = archived.clone();
    too_large.history = (1..=model::MAX_HISTORY_RECORDS)
        .map(|id| {
            let mut record = archived.history[0].clone();
            record.operation_id = format!("{id:032x}");
            record
                .native_completion
                .as_mut()
                .unwrap()
                .intent
                .operation_id = record.operation_id.clone();
            record
        })
        .collect();
    too_large.validate().unwrap();
    assert!(matches!(
        journal::encode_document(&too_large),
        Err(UpdateManagerError::JournalTooLarge)
    ));
    assert!(matches!(
        manager.state.store(&too_large, Some(archived.generation)),
        Err(UpdateManagerError::JournalTooLarge)
    ));
    assert_eq!(manager.state.load().unwrap().unwrap(), archived);
}

#[test]
fn native_idle_migration_is_fenced_replayable_and_blocks_legacy_mutations() {
    let fixture = Fixture::new();
    let confirmed = fixture.stage(1, 1, 1);
    let candidate = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&confirmed.slot_id).unwrap();
    let before = manager.state.load().unwrap().unwrap();
    assert!(
        manager
            .migrate_native_idle(before.generation + 1, |_| {
                panic!("stale generation must not consume credentials")
            })
            .is_err()
    );
    assert!(
        manager
            .migrate_native_idle(before.generation, |receipt| {
                assert!(receipt.is_none());
                Err(UpdateManagerError::RecoveryRequired)
            })
            .is_err()
    );
    assert_eq!(manager.state.load().unwrap().unwrap(), before);
    let generation = manager
        .migrate_native_idle(before.generation, |receipt| {
            assert!(receipt.is_none());
            Ok(())
        })
        .unwrap();
    let migrated = manager.state.load().unwrap().unwrap();
    assert_eq!(generation, before.generation + 1);
    assert_eq!(
        migrated.schema_version,
        model::NATIVE_LIFECYCLE_SCHEMA_VERSION
    );
    assert_eq!(migrated.floor_sequence, before.floor_sequence);
    assert_eq!(migrated.history, before.history);
    assert_eq!(fixture.current(), confirmed.slot_id);
    drop(manager);
    let manager = fixture.manager(1);
    assert_eq!(
        manager
            .migrate_native_idle(generation, |_| {
                panic!("already migrated replay must not clean unrelated runtime material")
            })
            .unwrap(),
        generation
    );
    assert!(manager.activate(request(1, &candidate, 1, 100)).is_err());
    assert!(
        manager
            .confirm_health([1; 16], [65; 32], clock(1, 101))
            .is_err()
    );
    assert!(manager.recover(clock(1, 101)).is_err());
    assert_eq!(manager.state.load().unwrap().unwrap(), migrated);
}

#[test]
fn native_migration_archives_exact_completion_and_refuses_unfinished_recovery() {
    let fixture = Fixture::new();
    let confirmed = fixture.stage(1, 1, 1);
    let candidate = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&confirmed.slot_id).unwrap();
    manager.activate(request(72, &candidate, 8, 100)).unwrap();
    let pending = manager.status().unwrap();
    assert!(
        manager
            .migrate_native_idle(pending.generation, |_| panic!("pending"))
            .is_err()
    );
    let intent = manager
        .begin_native_rejection(pending.generation, [72; 16])
        .unwrap();
    let rejected = manager.inspect_recovery().unwrap();
    assert!(
        manager
            .migrate_native_idle(rejected.generation, |_| panic!("rollback"))
            .is_err()
    );
    let restored = manager
        .restore_native_selection(rejected.generation, &intent, || Ok(()))
        .unwrap();
    assert!(
        manager
            .migrate_native_idle(restored.generation, |_| panic!("restoring"))
            .is_err()
    );
    let completion = manager
        .acknowledge_native_restoration(
            restored.generation,
            restored.rollback_intent.as_ref().unwrap(),
            || Ok(()),
        )
        .unwrap();
    let complete = manager.state.load().unwrap().unwrap();
    // A cleanup that completed before the process failed must be safely replayed.
    assert!(
        manager
            .migrate_native_idle(complete.generation, |receipt| {
                assert_eq!(receipt, Some(&completion));
                Err(UpdateManagerError::RecoveryRequired)
            })
            .is_err()
    );
    assert_eq!(manager.state.load().unwrap().unwrap(), complete);
    drop(manager);
    let manager = fixture.manager(1);
    let generation = manager
        .migrate_native_idle(complete.generation, |receipt| {
            assert_eq!(receipt, Some(&completion));
            Ok(())
        })
        .unwrap();
    let archived = manager.state.load().unwrap().unwrap();
    assert!(archived.native_completion.is_none());
    assert_eq!(
        archived.history[0].native_completion.as_ref(),
        Some(&completion)
    );
    assert_eq!(archived.history[0].outcome, complete.history[0].outcome);
    assert_eq!(archived.floor_sequence, complete.floor_sequence);
    assert_eq!(fixture.current(), confirmed.slot_id);
    assert!(
        manager
            .migrate_native_idle(complete.generation, |_| panic!("old cleanup"))
            .is_err()
    );
    assert_eq!(manager.inspect_recovery().unwrap().generation, generation);
    for damage in 0..8 {
        let mut broken = archived.clone();
        let receipt = broken.history[0].native_completion.as_mut().unwrap();
        match damage {
            0 => broken.schema_version = model::JOURNAL_SCHEMA_VERSION,
            1 => receipt.completion_generation = generation,
            2 => receipt.intent.intent_generation = receipt.completion_generation,
            3 => receipt.confirmed_slot_id = candidate.slot_id.clone(),
            4 => receipt.health_binding = "f".repeat(64),
            5 => receipt.intent.operation_id = "f".repeat(32),
            6 => broken.history[0].outcome = model::OperationOutcome::Confirmed,
            _ => receipt.intent.phase = NativeRollbackPhase::RollbackPending,
        }
        assert!(
            broken.validate().is_err(),
            "accepted damaged archive {damage}"
        );
    }
}

#[test]
fn native_migration_revalidates_selection_and_document_after_cleanup() {
    let fixture = Fixture::new();
    let confirmed = fixture.stage(1, 1, 1);
    let candidate = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&confirmed.slot_id).unwrap();
    let before = manager.state.load().unwrap().unwrap();
    assert!(
        manager
            .migrate_native_idle(before.generation, |_| {
                manager.pointer.switch(&candidate.slot_id)?;
                Ok(())
            })
            .is_err()
    );
    assert_eq!(manager.state.load().unwrap().unwrap(), before);
    manager.pointer.switch(&confirmed.slot_id).unwrap();
    assert!(
        manager
            .migrate_native_idle(before.generation, |_| {
                let mut changed = before.clone();
                changed.last_rollback = Some(RollbackReason::HealthTimeout);
                manager.state.store(&changed, Some(before.generation))?;
                Ok(())
            })
            .is_err()
    );
    assert_eq!(manager.state.load().unwrap().unwrap().schema_version, 1);
    let previous_generation = before.generation;
    let mut exhausted = before;
    exhausted.generation = u64::MAX;
    manager
        .state
        .store(&exhausted, Some(previous_generation))
        .unwrap();
    assert!(matches!(
        manager.migrate_native_idle(u64::MAX, |_| panic!("exhausted")),
        Err(UpdateManagerError::GenerationExhausted)
    ));
}

#[test]
fn pending_activation_rejects_same_boot_clock_reversal_without_mutation() {
    let fixture = Fixture::new();
    let previous = fixture.stage(1, 1, 1);
    let candidate = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&previous.slot_id).unwrap();
    let activation = request(1, &candidate, 1, 1_000);
    manager.activate(activation.clone()).unwrap();
    let before = manager.status().unwrap();
    let journal = fs::read(fixture.state.path().join("application-state.json")).unwrap();

    let mut reversed = activation;
    reversed.clock = clock(1, 999);
    assert!(matches!(
        manager.activate_with_quiescence(reversed, || panic!("replay must not stop services")),
        Err(UpdateManagerError::RecoveryRequired)
    ));
    assert!(matches!(
        manager.confirm_health([1; 16], [65; 32], clock(1, 999)),
        Err(UpdateManagerError::RecoveryRequired)
    ));
    assert_eq!(manager.status().unwrap(), before);
    assert_eq!(fixture.current(), candidate.slot_id);
    assert_eq!(
        fs::read(fixture.state.path().join("application-state.json")).unwrap(),
        journal
    );
    assert_eq!(
        manager
            .confirm_health([1; 16], [65; 32], clock(1, 1_000))
            .unwrap(),
        ConfirmationOutcome::Confirmed
    );
}

#[test]
fn quiescence_runs_after_durable_prepare_before_pointer_switch() {
    let fixture = Fixture::new();
    let previous = fixture.stage(1, 1, 1);
    let candidate = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&previous.slot_id).unwrap();
    let activation = request(1, &candidate, 1, 1_000);
    let called = std::cell::Cell::new(false);
    manager
        .activate_with_quiescence(activation.clone(), || {
            let status = manager.status()?;
            assert_eq!(status.pending.unwrap().phase, PendingPhaseStatus::Prepared);
            assert_eq!(status.active_slot_id, previous.slot_id);
            assert_eq!(fixture.current(), previous.slot_id);
            called.set(true);
            Ok(())
        })
        .unwrap();
    assert!(called.get());
    assert_eq!(fixture.current(), candidate.slot_id);
    assert_eq!(
        manager.status().unwrap().pending.unwrap().phase,
        PendingPhaseStatus::AwaitingHealth
    );
    manager
        .confirm_health([1; 16], [65; 32], clock(1, 2_000))
        .unwrap();
    assert_eq!(
        manager
            .activate_with_quiescence(activation, || {
                panic!("confirmed replay must not quiesce the running composition")
            })
            .unwrap(),
        ActivationOutcome::ReplayConfirmed
    );
}

#[test]
fn admitted_operation_executes_after_reopen_without_early_pointer_change() {
    let fixture = Fixture::new();
    let previous = fixture.stage(1, 1, 1);
    let candidate = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&previous.slot_id).unwrap();
    let mut activation = request(1, &candidate, 1, 1_000);
    assert_eq!(
        manager.admit_activation(&activation).unwrap(),
        ActivationPreparation::Prepared
    );
    assert_eq!(fixture.current(), previous.slot_id);
    assert_eq!(
        manager.status().unwrap().pending.unwrap().phase,
        PendingPhaseStatus::Prepared
    );
    drop(manager);
    let executor = fixture.manager(1);
    activation.clock = clock(1, 1_100);
    let called = std::cell::Cell::new(false);
    let outcome = executor
        .execute_prepared_activation(
            activation.clone(),
            || {
                called.set(true);
                assert_eq!(fixture.current(), previous.slot_id);
                Ok(())
            },
            || Ok(clock(1, 1_200)),
        )
        .unwrap();
    assert!(called.get());
    assert!(matches!(outcome, ActivationOutcome::AwaitingHealth { .. }));
    assert_eq!(fixture.current(), candidate.slot_id);
    assert_eq!(executor.status().unwrap().floor_sequence, 1);
    assert!(
        executor
            .execute_prepared_activation(
                activation,
                || panic!("execution replay must not quiesce"),
                || Ok(clock(1, 1_300))
            )
            .is_err()
    );
}

#[test]
fn prepared_executor_rejects_mismatched_and_stale_intent_before_quiescence() {
    let fixture = Fixture::new();
    let previous = fixture.stage(1, 1, 1);
    let candidate = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&previous.slot_id).unwrap();
    let activation = request(1, &candidate, 1, 1_000);
    manager.admit_activation(&activation).unwrap();
    let mut cases = Vec::new();
    let mut wrong = activation.clone();
    wrong.operation_id = [2; 16];
    cases.push(wrong);
    let mut wrong = activation.clone();
    wrong.health_binding = [2; 32];
    cases.push(wrong);
    let mut wrong = activation.clone();
    wrong.target_slot_id = previous.slot_id.clone();
    cases.push(wrong);
    for snapshot in [
        clock(2, 1_100),
        clock(1, 999),
        clock(1, 1_001 + DEFAULT_HEALTH_TIMEOUT_MS),
    ] {
        let mut wrong = activation.clone();
        wrong.clock = snapshot;
        cases.push(wrong);
    }
    for wrong in cases {
        assert!(
            manager
                .execute_prepared_activation(
                    wrong,
                    || panic!("invalid intent must not stop services"),
                    || panic!("clock must not be sampled after rejected admission")
                )
                .is_err()
        );
        assert_eq!(fixture.current(), previous.slot_id);
    }
}

#[test]
fn prepared_executor_rechecks_time_after_quiescence_and_retains_recovery_intent() {
    for after in [
        clock(2, 1_200),
        clock(1, 1_050),
        clock(1, 1_001 + DEFAULT_HEALTH_TIMEOUT_MS),
    ] {
        let fixture = Fixture::new();
        let previous = fixture.stage(1, 1, 1);
        let candidate = fixture.stage(2, 1, 1);
        let manager = fixture.manager(1);
        manager.initialize(&previous.slot_id).unwrap();
        let mut activation = request(1, &candidate, 1, 1_000);
        manager.admit_activation(&activation).unwrap();
        activation.clock = clock(1, 1_100);
        let called = std::cell::Cell::new(false);
        assert!(
            manager
                .execute_prepared_activation(
                    activation,
                    || {
                        called.set(true);
                        Ok(())
                    },
                    || Ok(after)
                )
                .is_err()
        );
        assert!(called.get());
        assert_eq!(fixture.current(), previous.slot_id);
        assert_eq!(
            manager.status().unwrap().pending.unwrap().phase,
            PendingPhaseStatus::Prepared
        );
        drop(manager);
        let recovered = fixture.manager(1);
        assert!(matches!(
            recovered.recover(clock(1, 2_000)).unwrap(),
            RecoveryOutcome::RolledBack { .. }
        ));
        assert_eq!(recovered.status().unwrap().floor_sequence, 1);
    }
}

#[test]
fn failed_quiescence_retains_intent_and_recovers_without_selecting_candidate() {
    let fixture = Fixture::new();
    let previous = fixture.stage(1, 1, 1);
    let candidate = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&previous.slot_id).unwrap();
    assert!(
        manager
            .activate_with_quiescence(request(1, &candidate, 1, 1_000), || {
                Err(UpdateManagerError::RecoveryRequired)
            })
            .is_err()
    );
    assert_eq!(fixture.current(), previous.slot_id);
    assert_eq!(
        manager.status().unwrap().pending.unwrap().phase,
        PendingPhaseStatus::Prepared
    );
    drop(manager);
    let reopened = fixture.manager(1);
    assert!(matches!(
        reopened.recover(clock(1, 2_000)).unwrap(),
        RecoveryOutcome::RolledBack {
            reason: RollbackReason::InterruptedPrepare,
            ..
        }
    ));
    assert_eq!(fixture.current(), previous.slot_id);
    let status = reopened.status().unwrap();
    assert!(status.pending.is_none());
    assert_eq!(status.floor_sequence, 1);
}

#[test]
fn status_metadata_reuses_the_held_slot_lock_without_mutating_state() {
    let fixture = Fixture::new();
    let slot = fixture.stage(1, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&slot.slot_id).unwrap();
    let before = manager.status().unwrap();
    for _ in 0..4 {
        let verified = manager.inspect_slot(&before.active_slot_id).unwrap();
        assert_eq!(verified.slot_id, slot.slot_id);
        assert_eq!(verified.manifest, slot.manifest);
        assert_eq!(manager.status().unwrap(), before);
        assert_eq!(fixture.current(), slot.slot_id);
    }
    assert!(manager.inspect_slot("../invalid").is_err());
    assert_eq!(manager.status().unwrap(), before);
}

#[test]
fn verified_slot_handles_are_exact_read_only_and_retain_file_identity() {
    use std::io::{Read, Write};
    use std::os::unix::fs::MetadataExt;
    let fixture = Fixture::new();
    let slot = fixture.stage(1, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&slot.slot_id).unwrap();
    let before = manager.status().unwrap();
    let handles = manager
        .open_verified_slot_files(
            &slot.slot_id,
            &["bin/bli-kvm-server", "share/web/index.html"],
        )
        .unwrap();
    assert_eq!(handles.slot().slot_id, slot.slot_id);
    assert_eq!(handles.files().len(), 2);
    let file = &handles.files()["bin/bli-kvm-server"];
    let path = slot.slot_path.join("bin/bli-kvm-server");
    let original = fs::read(&path).unwrap();
    let inode = file.metadata().unwrap().ino();
    assert_eq!(inode, fs::metadata(&path).unwrap().ino());
    assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o555);
    assert_eq!(
        handles.files()["share/web/index.html"]
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o444
    );
    let mut reader = file.try_clone().unwrap();
    assert!(reader.write_all(b"not permitted").is_err());
    assert_eq!(manager.status().unwrap(), before);
    for paths in [
        &[][..],
        &["../bin/bli-kvm-server"][..],
        &[".hardened-blikvm/manifest.json"][..],
        &["bin/bli-kvm-server", "bin/bli-kvm-server"][..],
    ] {
        assert!(
            manager
                .open_verified_slot_files(&slot.slot_id, paths)
                .is_err()
        );
    }
    // Authorized fixture tampering demonstrates why pathname reopens are not
    // interchangeable with a held identity. No installed slot is modified.
    fs::set_permissions(path.parent().unwrap(), Permissions::from_mode(0o755)).unwrap();
    fs::rename(&path, fixture.sources.path().join("retained-original")).unwrap();
    fs::write(&path, b"replacement").unwrap();
    assert_ne!(inode, fs::metadata(&path).unwrap().ino());
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, original);
    assert!(
        manager
            .open_verified_slot_files(&slot.slot_id, &["bin/bli-kvm-server"])
            .is_err()
    );
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
fn native_restoration_quiesces_before_selection_and_retains_pending_across_reopen() {
    for prepared_only in [true, false] {
        let fixture = Fixture::new();
        let previous = fixture.stage(1, 1, 1);
        let candidate = fixture.stage(2, 1, 1);
        let manager = fixture.manager(1);
        manager.initialize(&previous.slot_id).unwrap();
        let activation = request(35, &candidate, 5, 500);
        if prepared_only {
            manager.admit_activation(&activation).unwrap();
        } else {
            manager.activate(activation).unwrap();
        }
        let intent = manager
            .begin_native_rejection(manager.status().unwrap().generation, [35; 16])
            .unwrap();
        let generation = manager.inspect_recovery().unwrap().generation;
        let journal_path = fixture.state.path().join(journal::JOURNAL_FILE_NAME);
        let before = fs::read(&journal_path).unwrap();
        let selected_before = fixture.current();
        let mut wrong = intent.clone();
        wrong.phase = NativeRollbackPhase::Restoring;
        assert!(
            manager
                .restore_native_selection(generation, &wrong, || panic!(
                    "wrong intent must not quiesce"
                ))
                .is_err()
        );
        assert!(
            manager
                .restore_native_selection(generation + 1, &intent, || panic!(
                    "stale generation must not quiesce"
                ))
                .is_err()
        );
        assert!(
            manager
                .restore_native_selection(generation, &intent, || Err(
                    UpdateManagerError::RecoveryRequired
                ))
                .is_err()
        );
        assert_eq!(fs::read(&journal_path).unwrap(), before);
        assert_eq!(fixture.current(), selected_before);
        let anchor = previous.slot_path.join("bin/bli-kvm-server");
        let permissions = fs::metadata(&anchor).unwrap().permissions();
        assert!(
            manager
                .restore_native_selection(generation, &intent, || {
                    assert_eq!(fixture.current(), selected_before);
                    fs::set_permissions(&anchor, Permissions::from_mode(0o600)).unwrap();
                    Ok(())
                })
                .is_err()
        );
        assert_eq!(fixture.current(), selected_before);
        assert_eq!(fs::read(&journal_path).unwrap(), before);
        fs::set_permissions(&anchor, permissions).unwrap();
        // Reproduce interruption after pointer replacement but before journal
        // persistence. A new executor still has to quiesce; selection is no proof.
        manager.pointer.switch(&previous.slot_id).unwrap();
        drop(manager);
        let manager = fixture.manager(1);
        let mut stops = 0;
        let restored = manager
            .restore_native_selection(generation, &intent, || {
                stops += 1;
                assert_eq!(fs::read(&journal_path).unwrap(), before);
                Ok(())
            })
            .unwrap();
        assert_eq!(stops, 1);
        assert_eq!(fixture.current(), previous.slot_id);
        assert_eq!(restored.selection, RecoverySelection::Confirmed);
        assert!(restored.pending.is_some());
        let restoring = restored.rollback_intent.unwrap();
        assert_eq!(restoring.phase, NativeRollbackPhase::Restoring);
        let document = manager.state.load().unwrap().unwrap();
        assert_eq!(document.floor_sequence, 1);
        assert!(document.last_rollback.is_none());
        assert_eq!(
            document.history.last().unwrap().outcome,
            model::OperationOutcome::Pending
        );
        assert!(
            manager
                .restore_native_selection(generation, &intent, || panic!("replay must not quiesce"))
                .is_err()
        );
        drop(manager);
        let manager = fixture.manager(1);
        let resumed = manager
            .restore_native_selection(restored.generation, &restoring, || {
                stops += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(stops, 2);
        assert_eq!(resumed.generation, restored.generation + 1);
        assert!(resumed.pending.is_some());
        assert_eq!(manager.state.load().unwrap().unwrap().floor_sequence, 1);
        assert!(matches!(
            manager.recover(clock(9, 100_000)),
            Err(UpdateManagerError::RecoveryRequired)
        ));
    }
}

#[test]
fn native_rejection_is_durable_fenced_and_blocks_legacy_mutations() {
    let fixture = Fixture::new();
    let previous = fixture.stage(1, 1, 1);
    let candidate = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&previous.slot_id).unwrap();
    manager.activate(request(35, &candidate, 5, 500)).unwrap();
    let generation = manager.status().unwrap().generation;
    let journal_path = fixture.state.path().join(journal::JOURNAL_FILE_NAME);
    let before = fs::read(&journal_path).unwrap();
    let legacy: serde_json::Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(legacy["schemaVersion"], 1);
    assert!(legacy.get("nativeRollback").is_none());
    let roundtrip: JournalDocument = serde_json::from_slice(&before).unwrap();
    roundtrip.validate().unwrap();
    assert_eq!(serde_json::to_value(roundtrip).unwrap(), legacy);
    assert!(
        manager
            .begin_native_rejection(generation + 1, [35; 16])
            .is_err()
    );
    assert!(
        manager
            .begin_native_rejection(generation, [36; 16])
            .is_err()
    );
    assert_eq!(fs::read(&journal_path).unwrap(), before);
    fs::set_permissions(
        candidate.slot_path.join("bin/bli-kvm-server"),
        Permissions::from_mode(0o600),
    )
    .unwrap();
    let intent = manager
        .begin_native_rejection(generation, [35; 16])
        .unwrap();
    assert_eq!(intent.intent_generation, generation + 1);
    assert_eq!(intent.reason, RollbackReason::ExplicitHealthFailure);
    assert_eq!(fixture.current(), candidate.slot_id);
    let persisted = fs::read(&journal_path).unwrap();
    let document = manager.state.load().unwrap().unwrap();
    assert_eq!(document.schema_version, 2);
    assert_eq!(document.floor_sequence, 1);
    for change in 0..4 {
        let mut invalid = document.clone();
        match change {
            0 => invalid.schema_version = 1,
            1 => invalid.native_rollback = None,
            2 => invalid.native_rollback.as_mut().unwrap().operation_id = "ff".repeat(16),
            _ => {
                invalid.native_rollback.as_mut().unwrap().intent_generation = invalid.generation + 1
            }
        }
        assert!(invalid.validate().is_err());
    }
    drop(manager);
    let manager = fixture.manager(1);
    let inspected = manager.inspect_recovery().unwrap();
    assert_eq!(inspected.rollback_intent, Some(intent));
    assert_eq!(inspected.selection, RecoverySelection::PendingTarget);
    assert_eq!(inspected.confirmed_slot_id, previous.slot_id);
    assert!(matches!(
        manager.recover(clock(9, 100_000)),
        Err(UpdateManagerError::RecoveryRequired)
    ));
    assert!(matches!(
        manager.abort_missing_health_challenge(),
        Err(UpdateManagerError::RecoveryRequired)
    ));
    assert!(matches!(
        manager.reject_health([35; 16], [99; 32]),
        Err(UpdateManagerError::RecoveryRequired)
    ));
    assert!(matches!(
        manager.confirm_health([35; 16], [99; 32], clock(5, 501)),
        Err(UpdateManagerError::RecoveryRequired)
    ));
    assert!(
        manager
            .begin_native_rejection(generation + 1, [35; 16])
            .is_err()
    );
    assert_eq!(fs::read(&journal_path).unwrap(), persisted);
    assert_eq!(fixture.current(), candidate.slot_id);
}

#[test]
fn recovery_inspection_preserves_journal_and_pointer_with_a_broken_candidate() {
    let fixture = Fixture::new();
    let previous = fixture.stage(1, 1, 1);
    let candidate = fixture.stage(2, 1, 1);
    let unrelated = fixture.stage(3, 1, 1);
    let manager = fixture.manager(1);
    assert!(matches!(
        manager.inspect_recovery(),
        Err(UpdateManagerError::NotInitialized)
    ));
    manager.initialize(&previous.slot_id).unwrap();
    let initial = manager.inspect_recovery().unwrap();
    assert_eq!(initial.selection, RecoverySelection::Confirmed);
    assert!(initial.pending.is_none());
    let journal_path = fixture.state.path().join(journal::JOURNAL_FILE_NAME);
    manager
        .activate_with_quiescence(request(35, &candidate, 5, 500), || {
            let before = fs::read(&journal_path).unwrap();
            let prepared = manager.inspect_recovery().unwrap();
            assert_eq!(prepared.selection, RecoverySelection::Confirmed);
            assert_eq!(
                prepared.pending.unwrap().phase,
                PendingPhaseStatus::Prepared
            );
            assert_eq!(fs::read(&journal_path).unwrap(), before);
            assert_eq!(fixture.current(), previous.slot_id);
            Ok(())
        })
        .unwrap();
    fs::set_permissions(
        candidate.slot_path.join("bin/bli-kvm-server"),
        Permissions::from_mode(0o600),
    )
    .unwrap();
    assert!(manager.status().is_err());
    let journal_before = fs::read(&journal_path).unwrap();
    let current_path = fixture.slots.path().join("current");
    for (target, expected) in [
        (
            Some(candidate.slot_id.as_str()),
            RecoverySelection::PendingTarget,
        ),
        (
            Some(previous.slot_id.as_str()),
            RecoverySelection::Confirmed,
        ),
        (Some(unrelated.slot_id.as_str()), RecoverySelection::Other),
        (None, RecoverySelection::Missing),
        (Some("../escape"), RecoverySelection::Unsafe),
    ] {
        if fs::symlink_metadata(&current_path).is_ok() {
            fs::remove_file(&current_path).unwrap();
        }
        if let Some(target) = target {
            symlink(target, &current_path).unwrap();
        }
        let before = fs::read_link(&current_path).ok();
        let inspection = manager.inspect_recovery().unwrap();
        assert_eq!(inspection.selection, expected);
        assert_eq!(inspection.confirmed_slot_id, previous.slot_id);
        assert_eq!(
            inspection.pending.as_ref().unwrap().phase,
            PendingPhaseStatus::AwaitingHealth
        );
        assert_eq!(manager.inspect_recovery().unwrap(), inspection);
        assert_eq!(fs::read_link(&current_path).ok(), before);
        assert_eq!(fs::read(&journal_path).unwrap(), journal_before);
    }
    fs::set_permissions(
        previous.slot_path.join("bin/bli-kvm-server"),
        Permissions::from_mode(0o600),
    )
    .unwrap();
    assert!(manager.inspect_recovery().is_err());
    assert_eq!(fs::read(&journal_path).unwrap(), journal_before);
    assert_eq!(fixture.current(), "../escape");
}

#[test]
fn recovery_inspection_requires_confirmed_system_compatibility() {
    let fixture = Fixture::new();
    let previous = fixture.stage(1, 1, 2);
    let manager = fixture.manager(2);
    manager.initialize(&previous.slot_id).unwrap();
    drop(manager);
    let journal_path = fixture.state.path().join(journal::JOURNAL_FILE_NAME);
    let before = fs::read(&journal_path).unwrap();
    let manager = fixture.manager(1);
    assert!(matches!(
        manager.inspect_recovery(),
        Err(UpdateManagerError::MinimumSystemSequence)
    ));
    assert_eq!(fs::read(&journal_path).unwrap(), before);
    assert_eq!(fixture.current(), previous.slot_id);
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
fn manager_waits_for_a_short_lived_owner() {
    let fixture = Fixture::new();
    let manager = fixture.manager(1);
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        drop(manager);
    });

    let replacement = ApplicationUpdateManager::open(
        fixture.slots.path(),
        fixture.state.path(),
        fixture.trust.clone(),
        1,
        DEFAULT_HEALTH_TIMEOUT_MS,
    );
    release.join().unwrap();
    replacement.unwrap();
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
    assert!(matches!(
        manager.inspect_recovery(),
        Err(UpdateManagerError::UnsafeJournal)
    ));
}

#[test]
fn a_rollover_keeps_a_pending_record_however_old() {
    use crate::model::{
        JournalDocument, MAX_HISTORY_RECORDS, OperationOutcome, OperationRecord, SlotReference,
    };
    let reference = SlotReference {
        slot_id: format!("app-{:020}-{}", 1, "a".repeat(64)),
        sequence: 1,
        manifest_sha256: "a".repeat(64),
    };
    let mut document = JournalDocument::initial(reference);
    document.generation = 500;
    let record = |index: usize, outcome: OperationOutcome| OperationRecord {
        operation_id: format!("{:032x}", index + 1),
        target_slot_id: "x".into(),
        health_binding: "b".repeat(64),
        outcome,
        native_completion: None,
    };
    document.history.push(record(0, OperationOutcome::Pending));
    for index in 1..MAX_HISTORY_RECORDS {
        document.history.push(record(index, OperationOutcome::Noop));
    }
    document
        .begin_operation("f".repeat(32), "x".into(), "c".repeat(64))
        .unwrap();
    assert!(document.history.len() <= MAX_HISTORY_RECORDS);
    assert_eq!(document.history[0].outcome, OperationOutcome::Pending);
    assert_eq!(
        document.history.last().unwrap().operation_id,
        "f".repeat(32)
    );
    // The newest finished ones stay, the oldest went.
    assert!(
        document
            .find_record(&format!("{:032x}", MAX_HISTORY_RECORDS))
            .is_some()
    );
    assert!(document.find_record(&format!("{:032x}", 2)).is_none());
    assert_eq!(document.pruned_through_generation, 500);
}

#[test]
fn native_rollbacks_beyond_the_journals_size_are_all_admitted() {
    // A native rollback's record keeps its completion (about 800 bytes): the
    // 64 KiB journal filled at the 81st, long before the history's 128
    // records rolled over, and every later admission was refused (code
    // audit of 2026-10-20, AUD-03).
    let fixture = Fixture::new();
    let confirmed = fixture.stage(1, 1, 1);
    let candidate = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&confirmed.slot_id).unwrap();
    let mut generation = manager.migrate_native_idle(1, |_| Ok(())).unwrap();
    let mut now = 100;
    // Beyond the 81st and beyond the history's 128 records.
    for operation in 1..=130_u8 {
        now += 10;
        let activation = request(operation, &candidate, 1, now);
        assert_eq!(
            manager
                .admit_native_activation(generation, &activation)
                .unwrap_or_else(|error| panic!("rollback {operation}: {error:?}")),
            ActivationPreparation::Prepared
        );
        let prepared = manager.inspect_recovery().unwrap();
        manager
            .execute_native_prepared_activation(
                prepared.generation,
                activation,
                || Ok(()),
                || Ok(clock(1, now + 1)),
            )
            .unwrap();
        let pending = manager.inspect_recovery().unwrap();
        let intent = manager
            .begin_native_rejection(pending.generation, [operation; 16])
            .unwrap();
        let rejected = manager.inspect_recovery().unwrap();
        let restored = manager
            .restore_native_selection(rejected.generation, &intent, || Ok(()))
            .unwrap();
        manager
            .acknowledge_native_restoration(
                restored.generation,
                restored.rollback_intent.as_ref().unwrap(),
                || Ok(()),
            )
            .unwrap();
        let complete = manager.state.load().unwrap().unwrap();
        generation = manager
            .migrate_native_idle(complete.generation, |_| Ok(()))
            .unwrap();
    }
    let size = fs::metadata(fixture.state.path().join("application-state.json"))
        .unwrap()
        .len();
    assert!(size <= 64 * 1024, "{size} bytes");
}

#[test]
fn the_operation_history_rolls_over_keeping_the_newest_records() {
    let fixture = Fixture::new();
    let slot1 = fixture.stage(1, 1, 1);
    let slot2 = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&slot1.slot_id).unwrap();
    // More activations than the history holds (the test board had 80 of
    // 128 on 2026-10-18): none is refused for the history's size.
    // (192's health binding, 192 + 64, wraps to zero, which is refused.)
    for operation in (1..=200_u8).filter(|operation| *operation != 192) {
        assert_eq!(
            manager
                .activate(request(operation, &slot1, 7, u64::from(operation)))
                .unwrap(),
            ActivationOutcome::AlreadyActive
        );
    }
    // The newest is still a deterministic retry.
    assert_eq!(
        manager.activate(request(200, &slot1, 7, 201)).unwrap(),
        ActivationOutcome::AlreadyActive
    );
    // An update after the rollover goes through and is confirmed.
    manager.activate(request(201, &slot2, 8, 300)).unwrap();
    manager
        .confirm_health([201; 16], [201_u8.wrapping_add(64); 32], clock(8, 301))
        .unwrap();
    assert_eq!(manager.status().unwrap().confirmed_slot_id, slot2.slot_id);
    let journal: serde_json::Value = serde_json::from_slice(
        &fs::read(fixture.state.path().join("application-state.json")).unwrap(),
    )
    .unwrap();
    let history = journal["history"].as_array().unwrap();
    assert!(history.len() <= 128, "{} records", history.len());
    assert!(
        history
            .iter()
            .any(|record| record["operationId"] == "c8".repeat(16)),
        "the newest no-op was pruned"
    );
    assert!(journal["prunedThroughGeneration"].as_u64().unwrap() > 0);
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

#[test]
fn garbage_collection_runs_on_a_native_journal_at_rest() {
    let fixture = Fixture::new();
    let slot1 = fixture.stage(1, 1, 1);
    let slot2 = fixture.stage(2, 1, 1);
    let slot3 = fixture.stage(3, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&slot2.slot_id).unwrap();
    let before = manager.status().unwrap();
    manager
        .migrate_native_idle(before.generation, |_| Ok(()))
        .unwrap();
    // Observed on the board: the legacy-only read refused every collection.
    assert!(manager.status().is_err());
    let collected = manager.garbage_collect(0).unwrap();
    let mut deleted = collected.deleted_slot_ids;
    deleted.sort();
    assert_eq!(deleted, vec![slot1.slot_id, slot3.slot_id]);
    assert_eq!(
        manager.observed_status().unwrap().active_slot_id,
        slot2.slot_id
    );
}

#[test]
fn native_completion_requires_fresh_readiness_and_survives_reopen() {
    for prepared_only in [true, false] {
        let fixture = Fixture::new();
        let confirmed = fixture.stage(1, 1, 1);
        let candidate = fixture.stage(2, 1, 1);
        let manager = fixture.manager(1);
        manager.initialize(&confirmed.slot_id).unwrap();
        let activation = request(71, &candidate, 8, 100);
        if prepared_only {
            manager.admit_activation(&activation).unwrap();
        } else {
            manager.activate(activation).unwrap();
        }
        let generation = manager.status().unwrap().generation;
        let intent = manager
            .begin_native_rejection(generation, [71; 16])
            .unwrap();
        let rejected = manager.inspect_recovery().unwrap();
        assert!(
            manager
                .acknowledge_native_restoration(rejected.generation, &intent, || panic!(
                    "early readiness"
                ))
                .is_err()
        );
        let restored = manager
            .restore_native_selection(rejected.generation, &intent, || Ok(()))
            .unwrap();
        let restoring = restored.rollback_intent.as_ref().unwrap();
        assert!(
            manager
                .acknowledge_native_restoration(restored.generation - 1, restoring, || panic!(
                    "stale readiness"
                ))
                .is_err()
        );
        assert!(
            manager
                .acknowledge_native_restoration(restored.generation, restoring, || Err(
                    UpdateManagerError::RecoveryRequired
                ))
                .is_err()
        );
        assert_eq!(manager.inspect_recovery().unwrap(), restored);
        // A successful probe must not hide a changed selection during the gate.
        assert!(
            manager
                .acknowledge_native_restoration(restored.generation, restoring, || {
                    manager.pointer.switch(&candidate.slot_id)?;
                    Ok(())
                })
                .is_err()
        );
        manager.pointer.switch(&confirmed.slot_id).unwrap();
        let mut observed = false;
        let completion = manager
            .acknowledge_native_restoration(restored.generation, restoring, || {
                observed = true;
                Ok(())
            })
            .unwrap();
        assert!(observed);
        assert_eq!(completion.completion_generation, restored.generation + 1);
        assert_eq!(completion.failed_target_slot_id, candidate.slot_id);
        assert_eq!(completion.confirmed_slot_id, confirmed.slot_id);
        let document = manager.state.load().unwrap().unwrap();
        assert_eq!(document.floor_sequence, 1);
        assert_eq!(
            document.schema_version,
            model::NATIVE_JOURNAL_SCHEMA_VERSION
        );
        assert!(document.pending.is_none());
        assert!(document.native_rollback.is_none());
        assert_eq!(document.native_completion, Some(completion.clone()));
        assert_eq!(
            document
                .find_record(&completion.intent.operation_id)
                .unwrap()
                .health_binding,
            completion.health_binding
        );
        assert!(manager.status().is_err()); // Legacy paths remain fenced out.
        assert!(
            manager
                .acknowledge_native_restoration(restored.generation, restoring, || panic!(
                    "replayed readiness"
                ))
                .is_err()
        );
        drop(manager);
        let reopened = fixture.manager(1);
        let inspected = reopened.inspect_recovery().unwrap();
        assert_eq!(inspected.completion, Some(completion));
        assert!(inspected.pending.is_none());
        assert_eq!(inspected.selection, RecoverySelection::Confirmed);
        assert!(reopened.recover(clock(9, 1)).is_err());
    }
}

#[test]
fn native_completion_validator_rejects_mixed_or_substituted_evidence() {
    let fixture = Fixture::new();
    let confirmed = fixture.stage(1, 1, 1);
    let candidate = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&confirmed.slot_id).unwrap();
    manager.activate(request(72, &candidate, 8, 100)).unwrap();
    let intent = manager
        .begin_native_rejection(manager.status().unwrap().generation, [72; 16])
        .unwrap();
    let restored = manager
        .restore_native_selection(
            manager.inspect_recovery().unwrap().generation,
            &intent,
            || Ok(()),
        )
        .unwrap();
    let before = manager.state.load().unwrap().unwrap();
    manager
        .acknowledge_native_restoration(
            restored.generation,
            restored.rollback_intent.as_ref().unwrap(),
            || Ok(()),
        )
        .unwrap();
    let complete = manager.state.load().unwrap().unwrap();
    for damage in 0..11 {
        let mut broken = complete.clone();
        let receipt = broken.native_completion.as_mut().unwrap();
        match damage {
            0 => broken.native_rollback = before.native_rollback.clone(),
            1 => broken.pending = before.pending.clone(),
            2 => receipt.completion_generation -= 1,
            3 => receipt.intent.intent_generation = receipt.completion_generation,
            4 => receipt.confirmed_slot_id = candidate.slot_id.clone(),
            5 => receipt.failed_target_slot_id = confirmed.slot_id.clone(),
            6 => receipt.health_binding = "f".repeat(64),
            7 => receipt.intent.operation_id = "f".repeat(32),
            8 => broken.history[0].outcome = model::OperationOutcome::Pending,
            9 => broken.schema_version = model::JOURNAL_SCHEMA_VERSION,
            _ => broken.last_rollback = None,
        }
        assert!(
            broken.validate().is_err(),
            "accepted damaged receipt {damage}"
        );
    }
}

#[test]
fn native_watchdog_persists_observed_reason_before_any_pointer_repair() {
    for reason in [
        RollbackReason::InterruptedPrepare,
        RollbackReason::BootChanged,
        RollbackReason::HealthTimeout,
        RollbackReason::PointerMismatch,
        RollbackReason::TargetInvalid,
    ] {
        let fixture = Fixture::new();
        let confirmed = fixture.stage(1, 1, 1);
        let candidate = fixture.stage(2, 1, 1);
        let manager = fixture.manager(1);
        manager.initialize(&confirmed.slot_id).unwrap();
        let activation = request(73, &candidate, 8, 100);
        if reason == RollbackReason::InterruptedPrepare {
            manager.admit_activation(&activation).unwrap();
        } else {
            manager.activate(activation).unwrap();
        }
        let before = manager.state.load().unwrap().unwrap();
        let mut observed_clock = clock(8, 101);
        match reason {
            RollbackReason::BootChanged => observed_clock = clock(9, 1),
            RollbackReason::HealthTimeout => {
                observed_clock.monotonic_ms =
                    before.pending.as_ref().unwrap().deadline_monotonic_ms + 1
            }
            RollbackReason::PointerMismatch => manager.pointer.switch(&confirmed.slot_id).unwrap(),
            RollbackReason::TargetInvalid => fs::set_permissions(
                candidate.slot_path.join("bin/bli-kvm-server"),
                Permissions::from_mode(0o600),
            )
            .unwrap(),
            _ => {}
        }
        let pointer_before = fixture.current();
        assert!(
            manager
                .begin_native_recovery(before.generation - 1, observed_clock)
                .is_err()
        );
        assert_eq!(manager.state.load().unwrap().unwrap(), before);
        let intent = manager
            .begin_native_recovery(before.generation, observed_clock)
            .unwrap()
            .unwrap();
        assert_eq!(intent.reason, reason);
        assert_eq!(fixture.current(), pointer_before);
        let marked = manager.state.load().unwrap().unwrap();
        assert_eq!(marked.pending, before.pending);
        assert_eq!(marked.history, before.history);
        assert_eq!(marked.floor_sequence, 1);
        assert_eq!(
            manager
                .begin_native_recovery(marked.generation, clock(10, 1))
                .unwrap(),
            Some(intent.clone())
        );
        assert_eq!(manager.state.load().unwrap().unwrap(), marked);
        let restored = manager
            .restore_native_selection(marked.generation, &intent, || Ok(()))
            .unwrap();
        let completed = manager
            .acknowledge_native_restoration(
                restored.generation,
                restored.rollback_intent.as_ref().unwrap(),
                || Ok(()),
            )
            .unwrap();
        assert_eq!(completed.intent.reason, reason);
        assert!(
            manager
                .begin_native_recovery(completed.completion_generation, clock(11, 1))
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn native_watchdog_keeps_live_candidate_and_rejects_reversed_clock() {
    let fixture = Fixture::new();
    let confirmed = fixture.stage(1, 1, 1);
    let candidate = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&confirmed.slot_id).unwrap();
    manager.activate(request(74, &candidate, 8, 100)).unwrap();
    let before = manager.state.load().unwrap().unwrap();
    for time in [
        100,
        101,
        before.pending.as_ref().unwrap().deadline_monotonic_ms,
    ] {
        assert_eq!(
            manager
                .begin_native_recovery(before.generation, clock(8, time))
                .unwrap(),
            None
        );
        assert_eq!(manager.state.load().unwrap().unwrap(), before);
    }
    assert!(
        manager
            .begin_native_recovery(before.generation, clock(8, 99))
            .is_err()
    );
    assert_eq!(manager.state.load().unwrap().unwrap(), before);
    assert_eq!(fixture.current(), candidate.slot_id);
}

#[test]
fn journal_rejects_orphan_pending_history_in_each_activation_phase() {
    let fixture = Fixture::new();
    let confirmed = fixture.stage(1, 1, 1);
    let candidate = fixture.stage(2, 1, 1);
    let manager = fixture.manager(1);
    manager.initialize(&confirmed.slot_id).unwrap();
    let activation = request(75, &candidate, 8, 100);
    manager.admit_activation(&activation).unwrap();
    let prepared = manager.state.load().unwrap().unwrap();
    manager
        .execute_prepared_activation(activation, || Ok(()), || Ok(clock(8, 101)))
        .unwrap();
    let awaiting = manager.state.load().unwrap().unwrap();
    let intent = manager
        .begin_native_rejection(awaiting.generation, [75; 16])
        .unwrap();
    let rejected = manager.state.load().unwrap().unwrap();
    manager
        .restore_native_selection(rejected.generation, &intent, || Ok(()))
        .unwrap();
    let restoring = manager.state.load().unwrap().unwrap();
    for original in [prepared, awaiting, rejected, restoring] {
        original.validate().unwrap();
        let mut altered = original.clone();
        let mut orphan = altered.history[0].clone();
        orphan.operation_id = encode_hex(&[76; 16]);
        altered.history.push(orphan);
        assert!(matches!(
            altered.validate(),
            Err(UpdateManagerError::InvalidJournal)
        ));
        // A historical terminal operation is not an orphan pending operation.
        altered.history.last_mut().unwrap().outcome = model::OperationOutcome::RolledBack {
            reason: RollbackReason::ExplicitHealthFailure,
        };
        altered.validate().unwrap();
    }
}

#[test]
fn native_credential_loss_is_schema3_only_and_survives_restoration_and_archive() {
    for native in [false, true] {
        let fixture = Fixture::new();
        let confirmed = fixture.stage(1, 1, 1);
        let candidate = fixture.stage(2, 1, 1);
        let manager = fixture.manager(1);
        manager.initialize(&confirmed.slot_id).unwrap();
        let activation = request(91, &candidate, 1, 100);
        if native {
            let idle = manager.migrate_native_idle(1, |_| Ok(())).unwrap();
            manager.admit_native_activation(idle, &activation).unwrap();
            let generation = manager.inspect_recovery().unwrap().generation;
            manager
                .execute_native_prepared_activation(
                    generation,
                    activation.clone(),
                    || Ok(()),
                    || Ok(clock(1, 101)),
                )
                .unwrap();
        } else {
            manager.activate(activation.clone()).unwrap();
        }
        let before = manager.state.load().unwrap().unwrap();
        let pointer = manager.pointer.read().unwrap();
        let intent = manager
            .begin_native_recovery_with_credentials(
                before.generation,
                clock(1, 102),
                |op, binding, target| {
                    assert!(native, "legacy schema must bypass credential inspection");
                    assert_eq!(op, encode_hex(&activation.operation_id));
                    assert_eq!(binding, encode_hex(&activation.health_binding));
                    assert_eq!(target, candidate.slot_id);
                    Ok(NativeCredentialAvailability::Lost)
                },
            )
            .unwrap();
        if !native {
            assert!(intent.is_none());
            assert_eq!(manager.state.load().unwrap().unwrap(), before);
            continue;
        }
        let intent = intent.unwrap();
        assert_eq!(intent.reason, RollbackReason::HealthChallengeLost);
        let marked = manager.state.load().unwrap().unwrap();
        assert_eq!(marked.pending, before.pending);
        assert_eq!(marked.floor_sequence, before.floor_sequence);
        assert_eq!(manager.pointer.read().unwrap(), pointer);
        let mut invalid_legacy = marked.clone();
        invalid_legacy.schema_version = model::NATIVE_JOURNAL_SCHEMA_VERSION;
        assert!(invalid_legacy.validate().is_err());
        assert_eq!(
            manager
                .begin_native_recovery_with_credentials(
                    marked.generation,
                    clock(2, 1),
                    |_, _, _| panic!("existing intent")
                )
                .unwrap(),
            Some(intent.clone())
        );
        let restored = manager
            .restore_native_selection(marked.generation, &intent, || Ok(()))
            .unwrap();
        manager
            .acknowledge_native_restoration(
                restored.generation,
                restored.rollback_intent.as_ref().unwrap(),
                || Ok(()),
            )
            .unwrap();
        let complete = manager.inspect_recovery().unwrap();
        manager
            .migrate_native_idle(complete.generation, |_| Ok(()))
            .unwrap();
        let archived = manager.state.load().unwrap().unwrap();
        archived.validate().unwrap();
        assert_eq!(
            archived.history[0]
                .native_completion
                .as_ref()
                .unwrap()
                .intent
                .reason,
            RollbackReason::HealthChallengeLost
        );
    }
}

#[test]
fn native_credential_inspection_revalidates_and_preserves_reason_precedence() {
    for case in [
        "intact",
        "error",
        "document-change",
        "pointer-change",
        "timeout",
        "reverse-time",
    ] {
        let fixture = Fixture::new();
        let confirmed = fixture.stage(1, 1, 1);
        let candidate = fixture.stage(2, 1, 1);
        let manager = fixture.manager(1);
        manager.initialize(&confirmed.slot_id).unwrap();
        let idle = manager.migrate_native_idle(1, |_| Ok(())).unwrap();
        let activation = request(92, &candidate, 1, 100);
        manager.admit_native_activation(idle, &activation).unwrap();
        let prepared = manager.inspect_recovery().unwrap();
        manager
            .execute_native_prepared_activation(
                prepared.generation,
                activation,
                || Ok(()),
                || Ok(clock(1, 101)),
            )
            .unwrap();
        let before = manager.state.load().unwrap().unwrap();
        let now = match case {
            "timeout" => 60_101,
            "reverse-time" => 99,
            _ => 102,
        };
        let result = manager.begin_native_recovery_with_credentials(
            before.generation,
            clock(1, now),
            |_, _, _| match case {
                "timeout" | "reverse-time" => panic!("reason/clock must bypass callback"),
                "error" => Err(UpdateManagerError::ConfirmationMismatch),
                "document-change" => {
                    let mut changed = before.clone();
                    let previous = changed.advance_generation()?;
                    manager.state.store(&changed, Some(previous))?;
                    Ok(NativeCredentialAvailability::Lost)
                }
                "pointer-change" => {
                    manager.pointer.switch(&confirmed.slot_id)?;
                    Ok(NativeCredentialAvailability::Lost)
                }
                _ => Ok(NativeCredentialAvailability::Intact),
            },
        );
        if case == "timeout" {
            assert_eq!(
                result.unwrap().unwrap().reason,
                RollbackReason::HealthTimeout
            );
        } else if case == "intact" {
            assert!(result.unwrap().is_none());
            assert_eq!(manager.state.load().unwrap().unwrap(), before);
        } else {
            assert!(result.is_err(), "{case}");
            let after = manager.state.load().unwrap().unwrap();
            assert!(after.native_rollback.is_none());
            if case != "document-change" {
                assert_eq!(after, before);
            }
        }
    }
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
