use bli_kvm_system_boot_control::{
    ApplicationCompatibilityProof, BootControlError, BootSelector, CompatibilityError, JournalCopy,
    JournalPhase, PendingSelector, PendingSystemUpdate, Reconciliation, SelectedJournal,
    SelectedRecord, SelectedSelector, SelectorCopy, SelectorPendingState, SlotIdentity,
    SystemJournal, SystemRollbackReason, SystemSlot, reconcile, select_journal_copies,
    select_selector_copies, validate_application_confirmation,
    validate_confirmed_application_for_system_update,
};

const LAYOUT: &str = "layout-h616-recovery-test-v1";

fn hex(character: char, length: usize) -> String {
    std::iter::repeat_n(character, length).collect()
}

fn slot(slot: SystemSlot, sequence: u64, digest: char) -> SlotIdentity {
    SlotIdentity {
        slot,
        sequence,
        manifest_sha256: hex(digest, 64),
        layout_id: LAYOUT.into(),
    }
}

fn pending(phase: JournalPhase) -> PendingSystemUpdate {
    let boot_material = matches!(
        phase,
        JournalPhase::AwaitingHealth
            | JournalPhase::ConfirmationPrepared
            | JournalPhase::RollbackPrepared
    );
    PendingSystemUpdate {
        operation_id: hex('1', 32),
        previous: slot(SystemSlot::A, 5, 'a'),
        candidate: slot(SystemSlot::B, 6, 'b'),
        phase,
        initial_attempts: 3,
        remaining_attempts: if boot_material { 2 } else { 3 },
        selector_generation: matches!(
            phase,
            JournalPhase::BootArmed
                | JournalPhase::AwaitingHealth
                | JournalPhase::ConfirmationPrepared
                | JournalPhase::RollbackPrepared
        )
        .then_some(10),
        boot_id: boot_material.then(|| hex('2', 32)),
        health_binding: boot_material.then(|| hex('3', 64)),
        deadline_monotonic_ms: boot_material.then_some(90_000),
    }
}

fn journal(phase: Option<JournalPhase>) -> SystemJournal {
    SystemJournal {
        layout_id: LAYOUT.into(),
        floor_sequence: 5,
        confirmed: slot(SystemSlot::A, 5, 'a'),
        pending: phase.map(pending),
        last_rollback: None,
    }
}

fn selector_pending(state: SelectorPendingState, remaining_attempts: u8) -> PendingSelector {
    PendingSelector {
        operation_id: hex('1', 32),
        previous_slot: SystemSlot::A,
        candidate_slot: SystemSlot::B,
        candidate_manifest_sha256: hex('b', 64),
        initial_attempts: 3,
        remaining_attempts,
        state,
    }
}

fn selector(state: Option<SelectorPendingState>) -> BootSelector {
    BootSelector {
        layout_id: LAYOUT.into(),
        confirmed_slot: SystemSlot::A,
        confirmed_manifest_sha256: hex('a', 64),
        pending: state.map(|state| {
            let remaining = if state == SelectorPendingState::ArmedCandidate {
                3
            } else {
                2
            };
            selector_pending(state, remaining)
        }),
    }
}

fn selected_journal(phase: Option<JournalPhase>) -> SelectedJournal {
    SelectedRecord {
        generation: 20,
        payload: journal(phase),
        degraded: false,
    }
}

fn selected_selector(state: Option<SelectorPendingState>) -> SelectedSelector {
    SelectedRecord {
        generation: if state == Some(SelectorPendingState::CandidateBooted) {
            11
        } else {
            10
        },
        payload: selector(state),
        degraded: false,
    }
}

fn application(sequence: u64, minimum_system_sequence: u64) -> ApplicationCompatibilityProof {
    ApplicationCompatibilityProof {
        application_sequence: sequence,
        minimum_system_sequence,
        manifest_sha256: hex('d', 64),
    }
}

#[test]
fn journal_copy_is_canonical_checksummed_and_domain_separated() {
    let record = JournalCopy::new(7, journal(None)).unwrap();
    let bytes = record.to_bytes().unwrap();
    let decoded = JournalCopy::from_bytes(&bytes).unwrap();
    assert_eq!(decoded.generation(), 7);
    assert_eq!(decoded.payload(), &journal(None));

    let mut noncanonical = vec![b' '];
    noncanonical.extend_from_slice(&bytes);
    assert_eq!(
        JournalCopy::from_bytes(&noncanonical),
        Err(BootControlError::InvalidDocument)
    );

    let decoded_json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let checksum = decoded_json["checksum"].as_str().unwrap();
    let tampered = std::str::from_utf8(&bytes)
        .unwrap()
        .replacen(checksum, &hex('f', 64), 1)
        .into_bytes();
    assert_eq!(
        JournalCopy::from_bytes(&tampered),
        Err(BootControlError::InvalidChecksum)
    );
    assert_eq!(
        SelectorCopy::from_bytes(&bytes),
        Err(BootControlError::InvalidDocument)
    );
}

#[test]
fn redundant_copy_selection_handles_torn_newer_and_split_brain_states() {
    let generation_7 = JournalCopy::new(7, journal(None))
        .unwrap()
        .to_bytes()
        .unwrap();
    let selected = select_journal_copies(Some(&generation_7), Some(&generation_7)).unwrap();
    assert_eq!(selected.generation, 7);
    assert!(!selected.degraded);

    let selected = select_journal_copies(Some(&generation_7), Some(b"{")).unwrap();
    assert_eq!(selected.generation, 7);
    assert!(selected.degraded);

    let generation_8 = JournalCopy::new(8, journal(None))
        .unwrap()
        .to_bytes()
        .unwrap();
    let selected = select_journal_copies(Some(&generation_7), Some(&generation_8)).unwrap();
    assert_eq!(selected.generation, 8);
    assert!(selected.degraded);

    let generation_9 = JournalCopy::new(9, journal(None))
        .unwrap()
        .to_bytes()
        .unwrap();
    assert_eq!(
        select_journal_copies(Some(&generation_7), Some(&generation_9)),
        Err(BootControlError::GenerationGap)
    );

    let mut alternate = journal(None);
    alternate.floor_sequence = 6;
    alternate.confirmed = slot(SystemSlot::B, 6, 'b');
    let divergent = JournalCopy::new(7, alternate).unwrap().to_bytes().unwrap();
    assert_eq!(
        select_journal_copies(Some(&generation_7), Some(&divergent)),
        Err(BootControlError::SplitBrain)
    );
    assert_eq!(
        select_journal_copies(Some(b"{"), None),
        Err(BootControlError::NoValidCopy)
    );
}

#[test]
fn selector_copies_apply_the_same_redundancy_rules() {
    let generation_4 = SelectorCopy::new(4, selector(None))
        .unwrap()
        .to_bytes()
        .unwrap();
    let generation_5 = SelectorCopy::new(5, selector(Some(SelectorPendingState::ArmedCandidate)))
        .unwrap()
        .to_bytes()
        .unwrap();
    let selected = select_selector_copies(Some(&generation_4), Some(&generation_5)).unwrap();
    assert_eq!(selected.generation, 5);
    assert!(selected.degraded);
    assert_eq!(
        selected.payload.pending.unwrap().state,
        SelectorPendingState::ArmedCandidate
    );
}

#[test]
fn invalid_journal_phase_material_and_attempts_are_rejected() {
    let mut same_slot = journal(Some(JournalPhase::WritePrepared));
    same_slot.pending.as_mut().unwrap().candidate.slot = SystemSlot::A;
    assert_eq!(
        JournalCopy::new(1, same_slot),
        Err(BootControlError::InvalidJournal)
    );

    let mut premature_boot = journal(Some(JournalPhase::CandidateReady));
    premature_boot.pending.as_mut().unwrap().boot_id = Some(hex('2', 32));
    assert_eq!(
        JournalCopy::new(1, premature_boot),
        Err(BootControlError::InvalidJournal)
    );

    let mut no_decrement = journal(Some(JournalPhase::AwaitingHealth));
    no_decrement.pending.as_mut().unwrap().remaining_attempts = 3;
    assert_eq!(
        JournalCopy::new(1, no_decrement),
        Err(BootControlError::InvalidJournal)
    );

    let mut zero_binding = journal(Some(JournalPhase::AwaitingHealth));
    zero_binding.pending.as_mut().unwrap().health_binding = Some(hex('0', 64));
    assert_eq!(
        JournalCopy::new(1, zero_binding),
        Err(BootControlError::InvalidJournal)
    );
}

#[test]
fn invalid_selector_attempt_transitions_are_rejected() {
    let mut booted_without_decrement = selector(Some(SelectorPendingState::CandidateBooted));
    booted_without_decrement
        .pending
        .as_mut()
        .unwrap()
        .remaining_attempts = 3;
    assert_eq!(
        SelectorCopy::new(1, booted_without_decrement),
        Err(BootControlError::InvalidSelector)
    );

    let mut wrong_previous = selector(Some(SelectorPendingState::ArmedCandidate));
    wrong_previous.pending.as_mut().unwrap().previous_slot = SystemSlot::B;
    assert_eq!(
        SelectorCopy::new(1, wrong_previous),
        Err(BootControlError::InvalidSelector)
    );
}

#[test]
fn reconciliation_covers_clean_write_ready_and_boot_arm_crash_points() {
    assert_eq!(
        reconcile(&selected_journal(None), &selected_selector(None)).unwrap(),
        Reconciliation::Clean
    );
    for phase in [JournalPhase::WritePrepared, JournalPhase::CandidateReady] {
        assert_eq!(
            reconcile(&selected_journal(Some(phase)), &selected_selector(None)).unwrap(),
            Reconciliation::CandidateUnselected { phase }
        );
    }
    assert_eq!(
        reconcile(
            &selected_journal(Some(JournalPhase::BootPrepared)),
            &selected_selector(None),
        )
        .unwrap(),
        Reconciliation::InterruptedArm
    );
    assert_eq!(
        reconcile(
            &selected_journal(Some(JournalPhase::BootPrepared)),
            &selected_selector(Some(SelectorPendingState::ArmedCandidate)),
        )
        .unwrap(),
        Reconciliation::FinishJournalBootArm
    );
}

#[test]
fn reconciliation_covers_attempt_health_confirmation_and_rollback() {
    assert_eq!(
        reconcile(
            &selected_journal(Some(JournalPhase::BootArmed)),
            &selected_selector(Some(SelectorPendingState::ArmedCandidate)),
        )
        .unwrap(),
        Reconciliation::ContinueArmed
    );
    assert_eq!(
        reconcile(
            &selected_journal(Some(JournalPhase::BootArmed)),
            &selected_selector(Some(SelectorPendingState::CandidateBooted)),
        )
        .unwrap(),
        Reconciliation::ObserveCandidateBoot
    );
    assert_eq!(
        reconcile(
            &selected_journal(Some(JournalPhase::AwaitingHealth)),
            &selected_selector(Some(SelectorPendingState::CandidateBooted)),
        )
        .unwrap(),
        Reconciliation::ContinueAwaitingHealth
    );
    assert_eq!(
        reconcile(
            &selected_journal(Some(JournalPhase::AwaitingHealth)),
            &selected_selector(Some(SelectorPendingState::FallbackPrevious)),
        )
        .unwrap(),
        Reconciliation::RecordRollback
    );

    assert_eq!(
        reconcile(
            &selected_journal(Some(JournalPhase::ConfirmationPrepared)),
            &selected_selector(Some(SelectorPendingState::CandidateBooted)),
        )
        .unwrap(),
        Reconciliation::SelectCandidateConfirmation
    );
    let candidate_confirmed = SelectedRecord {
        generation: 12,
        payload: BootSelector {
            layout_id: LAYOUT.into(),
            confirmed_slot: SystemSlot::B,
            confirmed_manifest_sha256: hex('b', 64),
            pending: None,
        },
        degraded: false,
    };
    assert_eq!(
        reconcile(
            &selected_journal(Some(JournalPhase::ConfirmationPrepared)),
            &candidate_confirmed,
        )
        .unwrap(),
        Reconciliation::FinishJournalConfirmation
    );

    assert_eq!(
        reconcile(
            &selected_journal(Some(JournalPhase::RollbackPrepared)),
            &selected_selector(Some(SelectorPendingState::CandidateBooted)),
        )
        .unwrap(),
        Reconciliation::SelectPreviousRollback
    );
    assert_eq!(
        reconcile(
            &selected_journal(Some(JournalPhase::RollbackPrepared)),
            &selected_selector(Some(SelectorPendingState::FallbackPrevious)),
        )
        .unwrap(),
        Reconciliation::FinishJournalRollback
    );
}

#[test]
fn reconciliation_rejects_mismatch_generation_regression_and_attempt_increase() {
    let mut wrong_layout = selected_selector(None);
    wrong_layout.payload.layout_id = "layout-other-v1".into();
    assert_eq!(
        reconcile(&selected_journal(None), &wrong_layout),
        Err(BootControlError::StateMismatch)
    );

    let mut stale_selector = selected_selector(Some(SelectorPendingState::ArmedCandidate));
    stale_selector.generation = 9;
    assert_eq!(
        reconcile(
            &selected_journal(Some(JournalPhase::BootArmed)),
            &stale_selector,
        ),
        Err(BootControlError::StateMismatch)
    );

    let mut journal = selected_journal(Some(JournalPhase::AwaitingHealth));
    journal.payload.pending.as_mut().unwrap().remaining_attempts = 1;
    let selector = selected_selector(Some(SelectorPendingState::CandidateBooted));
    assert_eq!(
        reconcile(&journal, &selector),
        Err(BootControlError::AttemptCounterIncreased)
    );

    let mut wrong_operation = selected_selector(Some(SelectorPendingState::CandidateBooted));
    wrong_operation
        .payload
        .pending
        .as_mut()
        .unwrap()
        .operation_id = hex('4', 32);
    assert_eq!(
        reconcile(
            &selected_journal(Some(JournalPhase::AwaitingHealth)),
            &wrong_operation,
        ),
        Err(BootControlError::StateMismatch)
    );
}

#[test]
fn rollback_reason_serialization_is_bounded_and_known() {
    let mut journal = journal(None);
    journal.last_rollback = Some(SystemRollbackReason::AttemptsExhausted);
    let bytes = JournalCopy::new(1, journal).unwrap().to_bytes().unwrap();
    assert!(
        std::str::from_utf8(&bytes)
            .unwrap()
            .contains("attempts_exhausted")
    );
    JournalCopy::from_bytes(&bytes).unwrap();
}

#[test]
fn application_confirmation_preserves_the_system_rollback_anchor() {
    let stable = journal(None);
    validate_confirmed_application_for_system_update(&stable, &application(9, 5)).unwrap();
    validate_application_confirmation(&stable, &application(10, 5)).unwrap();
    assert_eq!(
        validate_application_confirmation(&stable, &application(10, 6)),
        Err(CompatibilityError::IncompatibleWithConfirmedSystem)
    );

    for phase in [
        JournalPhase::WritePrepared,
        JournalPhase::CandidateReady,
        JournalPhase::BootPrepared,
        JournalPhase::BootArmed,
        JournalPhase::AwaitingHealth,
        JournalPhase::ConfirmationPrepared,
        JournalPhase::RollbackPrepared,
    ] {
        let pending = journal(Some(phase));
        validate_confirmed_application_for_system_update(&pending, &application(9, 5)).unwrap();
        validate_application_confirmation(&pending, &application(10, 5)).unwrap();
        assert_eq!(
            validate_application_confirmation(&pending, &application(10, 6)),
            Err(CompatibilityError::IncompatibleWithRollbackAnchor)
        );
    }

    let confirmed_candidate = SystemJournal {
        layout_id: LAYOUT.into(),
        floor_sequence: 6,
        confirmed: slot(SystemSlot::B, 6, 'b'),
        pending: None,
        last_rollback: None,
    };
    validate_application_confirmation(&confirmed_candidate, &application(10, 6)).unwrap();
}

#[test]
fn compatibility_rejects_invalid_application_or_system_evidence() {
    assert_eq!(
        validate_application_confirmation(&journal(None), &application(0, 1)),
        Err(CompatibilityError::InvalidApplicationProof)
    );
    let mut zero_digest = application(1, 1);
    zero_digest.manifest_sha256 = hex('0', 64);
    assert_eq!(
        validate_application_confirmation(&journal(None), &zero_digest),
        Err(CompatibilityError::InvalidApplicationProof)
    );

    let mut invalid_journal = journal(None);
    invalid_journal.floor_sequence = 4;
    assert_eq!(
        validate_application_confirmation(&invalid_journal, &application(1, 1)),
        Err(CompatibilityError::InvalidSystemState)
    );
}
