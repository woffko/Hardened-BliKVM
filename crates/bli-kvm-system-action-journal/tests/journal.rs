use bli_kvm_system_action_journal::{
    CopyIndex, JOURNAL_CHECKSUM_BYTES, JOURNAL_CHECKSUM_OFFSET, JOURNAL_COPY_BYTES,
    JOURNAL_COPY_HEADER_BYTES, JOURNAL_DOMAIN, JournalCopy, JournalError, POLICY_STATE_BYTES,
    POLICY_STATE_HEADER_BYTES, RawJournalCopy, decode_policy_state, encode_policy_state,
    plan_initialize, plan_repair, plan_update, select_copies, verify_written,
};
use bli_kvm_system_action_policy::{
    MAX_OPERATION_RECORDS, SystemAction, SystemActionPhase, SystemActionPolicyState,
    SystemActionRecordSnapshot,
};
use sha2::{Digest, Sha256};

const BOOT_A: [u8; 16] = [0xA1; 16];
const BOOT_B: [u8; 16] = [0xB2; 16];

fn record_snapshot(action: SystemAction, phase: SystemActionPhase) -> SystemActionRecordSnapshot {
    let (dispatch_started_at_ms, terminal_at_ms, terminal_boot_binding) = match phase {
        SystemActionPhase::Prepared => (None, None, None),
        SystemActionPhase::Dispatching => (Some(60_002), None, None),
        SystemActionPhase::PostBootAccepted => (Some(60_002), Some(30_000), Some(BOOT_B)),
        SystemActionPhase::Rejected => (Some(60_002), Some(60_003), Some(BOOT_A)),
    };
    SystemActionRecordSnapshot {
        operation_id: [1; 16],
        session_binding: 1,
        confirmation_binding: [2; 32],
        confirmation_issued_at_ms: 60_000,
        origin_boot_binding: BOOT_A,
        action,
        phase,
        prepared_at_ms: 60_001,
        dispatch_started_at_ms,
        terminal_at_ms,
        terminal_boot_binding,
    }
}

fn state(
    generation: u64,
    action: SystemAction,
    phase: SystemActionPhase,
) -> SystemActionPolicyState {
    let mut records = [None; MAX_OPERATION_RECORDS];
    records[0] = Some(record_snapshot(action, phase));
    SystemActionPolicyState::from_record_snapshots(generation, records).unwrap()
}

fn empty_state_with_generation(generation: u64) -> SystemActionPolicyState {
    SystemActionPolicyState::from_record_snapshots(generation, [None; MAX_OPERATION_RECORDS])
        .unwrap()
}

fn encoded_copy(state: SystemActionPolicyState) -> RawJournalCopy {
    JournalCopy::new(state).unwrap().to_bytes().unwrap()
}

fn reseal(bytes: &mut RawJournalCopy) {
    let mut hasher = Sha256::new();
    hasher.update(JOURNAL_DOMAIN);
    hasher.update(&bytes[..JOURNAL_CHECKSUM_OFFSET]);
    let digest = hasher.finalize();
    bytes[JOURNAL_CHECKSUM_OFFSET..].copy_from_slice(&digest);
}

#[test]
fn policy_state_codec_is_fixed_canonical_and_round_trips() {
    assert_eq!(POLICY_STATE_BYTES, 2_026);
    assert_eq!(JOURNAL_COPY_BYTES, 2_076);
    for original in [
        SystemActionPolicyState::empty(),
        state(1, SystemAction::Reboot, SystemActionPhase::Prepared),
        state(2, SystemAction::Shutdown, SystemActionPhase::Dispatching),
        state(3, SystemAction::Reboot, SystemActionPhase::PostBootAccepted),
        state(3, SystemAction::Shutdown, SystemActionPhase::Rejected),
    ] {
        let bytes = encode_policy_state(&original).unwrap();
        assert_eq!(bytes.len(), POLICY_STATE_BYTES);
        assert_eq!(decode_policy_state(&bytes).unwrap(), original);
        assert_eq!(
            encode_policy_state(&decode_policy_state(&bytes).unwrap()).unwrap(),
            bytes
        );
    }
    assert_eq!(
        decode_policy_state(&[0; POLICY_STATE_BYTES - 1]),
        Err(JournalError::InvalidLength)
    );
}

#[test]
fn every_single_byte_copy_corruption_is_rejected() {
    let bytes = encoded_copy(state(
        3,
        SystemAction::Reboot,
        SystemActionPhase::PostBootAccepted,
    ));
    assert_eq!(
        JournalCopy::from_bytes(&bytes).unwrap().to_bytes().unwrap(),
        bytes
    );
    for index in 0..JOURNAL_COPY_BYTES {
        let mut corrupt = bytes;
        corrupt[index] ^= 0x01;
        assert!(
            JournalCopy::from_bytes(&corrupt).is_err(),
            "byte {index} was not covered"
        );
    }
    assert_eq!(
        JournalCopy::from_bytes(&bytes[..JOURNAL_COPY_BYTES - 1]),
        Err(JournalError::InvalidLength)
    );
}

#[test]
fn valid_checksum_does_not_make_noncanonical_payload_valid() {
    let mut unused_slot = encoded_copy(SystemActionPolicyState::empty());
    let unused_padding = JOURNAL_COPY_HEADER_BYTES + POLICY_STATE_HEADER_BYTES + 1;
    unused_slot[unused_padding] = 1;
    reseal(&mut unused_slot);
    assert_eq!(
        JournalCopy::from_bytes(&unused_slot),
        Err(JournalError::InvalidEncoding)
    );

    let mut unknown_action =
        encoded_copy(state(1, SystemAction::Reboot, SystemActionPhase::Prepared));
    let record_start = JOURNAL_COPY_HEADER_BYTES + POLICY_STATE_HEADER_BYTES;
    let action_offset = record_start + 1 + 16 + 8 + 32 + 8 + 16;
    unknown_action[action_offset] = 9;
    reseal(&mut unknown_action);
    assert_eq!(
        JournalCopy::from_bytes(&unknown_action),
        Err(JournalError::InvalidEncoding)
    );

    let mut mismatched_generation = encoded_copy(state(
        1,
        SystemAction::Shutdown,
        SystemActionPhase::Prepared,
    ));
    mismatched_generation[10..18].copy_from_slice(&99u64.to_be_bytes());
    reseal(&mut mismatched_generation);
    assert_eq!(
        JournalCopy::from_bytes(&mismatched_generation),
        Err(JournalError::GenerationMismatch)
    );
}

#[test]
fn initialization_and_update_order_preserve_one_valid_generation() {
    let initial = plan_initialize(SystemActionPolicyState::empty()).unwrap();
    assert_eq!(initial.expected_generation(), 0);
    assert_eq!(initial.generation(), 1);
    assert_eq!(initial.first(), CopyIndex::A);
    assert_eq!(initial.second(), CopyIndex::B);

    let selected = select_copies(Some(initial.bytes()), Some(initial.bytes()), 1).unwrap();
    assert!(!selected.degraded());
    assert_eq!(selected.selected_index(), CopyIndex::A);
    assert_eq!(selected.state(), SystemActionPolicyState::empty());

    let prepared = state(1, SystemAction::Reboot, SystemActionPhase::Prepared);
    let update = plan_update(&selected, 1, prepared).unwrap();
    assert_eq!(update.expected_generation(), 1);
    assert_eq!(update.generation(), 2);
    assert_eq!(update.first(), CopyIndex::B);
    assert_eq!(update.second(), CopyIndex::A);

    let after_first = select_copies(Some(initial.bytes()), Some(update.bytes()), 1).unwrap();
    assert!(after_first.degraded());
    assert_eq!(after_first.selected_index(), CopyIndex::B);
    assert_eq!(after_first.repair_target(), Some(CopyIndex::A));
    assert_eq!(after_first.state(), prepared);

    let converged = select_copies(Some(update.bytes()), Some(update.bytes()), 2).unwrap();
    assert!(!converged.degraded());
    assert_eq!(converged.generation(), 2);
    assert_eq!(converged.state(), prepared);
    verify_written(update.bytes(), update.bytes()).unwrap();
}

#[test]
fn torn_first_write_falls_back_and_torn_second_keeps_new_state() {
    let initial = plan_initialize(SystemActionPolicyState::empty()).unwrap();
    let selected = select_copies(Some(initial.bytes()), Some(initial.bytes()), 1).unwrap();
    let prepared = state(1, SystemAction::Shutdown, SystemActionPhase::Prepared);
    let update = plan_update(&selected, 1, prepared).unwrap();
    let torn_new = &update.bytes()[..JOURNAL_COPY_BYTES / 2];

    let first_torn = select_copies(Some(initial.bytes()), Some(torn_new), 1).unwrap();
    assert!(first_torn.degraded());
    assert_eq!(first_torn.selected_index(), CopyIndex::A);
    assert_eq!(first_torn.state(), SystemActionPolicyState::empty());

    let second_torn = select_copies(Some(torn_new), Some(update.bytes()), 1).unwrap();
    assert!(second_torn.degraded());
    assert_eq!(second_torn.selected_index(), CopyIndex::B);
    assert_eq!(second_torn.state(), prepared);
}

#[test]
fn split_brain_generation_gap_and_rollback_floor_are_explicit() {
    let reboot = encoded_copy(state(1, SystemAction::Reboot, SystemActionPhase::Prepared));
    let shutdown = encoded_copy(state(
        1,
        SystemAction::Shutdown,
        SystemActionPhase::Prepared,
    ));
    assert_eq!(
        select_copies(Some(&reboot), Some(&shutdown), 0),
        Err(JournalError::SplitBrain)
    );

    let generation_one = encoded_copy(SystemActionPolicyState::empty());
    let generation_three = encoded_copy(state(
        2,
        SystemAction::Reboot,
        SystemActionPhase::Dispatching,
    ));
    assert_eq!(
        select_copies(Some(&generation_one), Some(&generation_three), 0),
        Err(JournalError::GenerationGap)
    );
    assert_eq!(
        select_copies(Some(&generation_one), Some(&generation_one), 2),
        Err(JournalError::RollbackDetected)
    );
    assert_eq!(select_copies(None, None, 0), Err(JournalError::NoValidCopy));
}

#[test]
fn repair_plan_copies_only_the_selected_generation() {
    let initial = plan_initialize(SystemActionPolicyState::empty()).unwrap();
    let selected = select_copies(Some(initial.bytes()), None, 1).unwrap();
    assert!(selected.degraded());
    let repair = plan_repair(&selected).unwrap().unwrap();
    assert_eq!(repair.expected_generation(), 1);
    assert_eq!(repair.generation(), 1);
    assert_eq!(repair.target(), CopyIndex::B);
    assert_eq!(repair.bytes(), initial.bytes());

    let repaired = select_copies(Some(initial.bytes()), Some(repair.bytes()), 1).unwrap();
    assert!(!repaired.degraded());
    assert!(plan_repair(&repaired).unwrap().is_none());
}

#[test]
fn stale_or_skipped_update_plans_fail_before_producing_bytes() {
    let initial = plan_initialize(SystemActionPolicyState::empty()).unwrap();
    let selected = select_copies(Some(initial.bytes()), Some(initial.bytes()), 1).unwrap();
    let prepared = state(1, SystemAction::Reboot, SystemActionPhase::Prepared);
    assert_eq!(
        plan_update(&selected, 2, prepared),
        Err(JournalError::ConcurrentStateChange)
    );
    assert_eq!(
        plan_update(
            &selected,
            1,
            state(2, SystemAction::Reboot, SystemActionPhase::Dispatching,),
        ),
        Err(JournalError::StateGenerationGap)
    );
    assert_eq!(
        plan_initialize(prepared),
        Err(JournalError::StateGenerationGap)
    );
}

#[test]
fn verification_and_generation_exhaustion_fail_closed() {
    let initial = plan_initialize(SystemActionPolicyState::empty()).unwrap();
    let mut corrupt = *initial.bytes();
    corrupt[JOURNAL_CHECKSUM_OFFSET] ^= 1;
    assert_eq!(
        verify_written(initial.bytes(), &corrupt),
        Err(JournalError::InvalidChecksum)
    );

    let exhausted = empty_state_with_generation(u64::MAX);
    assert_eq!(
        JournalCopy::new(exhausted),
        Err(JournalError::GenerationExhausted)
    );

    let last = empty_state_with_generation(u64::MAX - 1);
    let last_bytes = encoded_copy(last);
    let selected = select_copies(Some(&last_bytes), Some(&last_bytes), u64::MAX).unwrap();
    assert_eq!(
        plan_update(&selected, u64::MAX, exhausted),
        Err(JournalError::GenerationExhausted)
    );
}

#[test]
fn public_format_constants_match_checksum_layout() {
    assert_eq!(JOURNAL_CHECKSUM_BYTES, 32);
    assert_eq!(
        JOURNAL_CHECKSUM_OFFSET + JOURNAL_CHECKSUM_BYTES,
        JOURNAL_COPY_BYTES
    );
    assert_eq!(
        JOURNAL_COPY_HEADER_BYTES + POLICY_STATE_BYTES,
        JOURNAL_CHECKSUM_OFFSET
    );
}
