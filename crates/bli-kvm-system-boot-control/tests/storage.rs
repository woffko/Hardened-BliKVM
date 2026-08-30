use bli_kvm_system_boot_control::{
    BootSelector, CopyIndex, JournalCopy, JournalPhase, PendingSelector, PendingSystemUpdate,
    RecordDomain, RedundantRecordStorage, SelectorPendingState, SlotIdentity,
    StorageOperationError, SystemJournal, SystemSlot, initialize_journal_storage,
    initialize_selector_storage, load_journal_storage, load_selector_storage,
    persist_journal_storage, persist_selector_storage,
};

const LAYOUT: &str = "layout-h616-storage-test-v1";

#[derive(Debug, Clone, Default)]
struct CopyState {
    durable: Option<Vec<u8>>,
    volatile: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy)]
enum WriteFailure {
    Error,
    Tear,
}

#[derive(Debug, Clone, Default)]
struct MemoryStorage {
    journal: [CopyState; 2],
    selector: [CopyState; 2],
    writes: usize,
    syncs: usize,
    fail_write: Option<(usize, WriteFailure)>,
    fail_sync: Option<usize>,
    fail_read: bool,
}

impl MemoryStorage {
    fn state_mut(&mut self, domain: RecordDomain, index: CopyIndex) -> &mut CopyState {
        let states = match domain {
            RecordDomain::Journal => &mut self.journal,
            RecordDomain::Selector => &mut self.selector,
        };
        &mut states[match index {
            CopyIndex::A => 0,
            CopyIndex::B => 1,
        }]
    }

    fn state(&self, domain: RecordDomain, index: CopyIndex) -> &CopyState {
        let states = match domain {
            RecordDomain::Journal => &self.journal,
            RecordDomain::Selector => &self.selector,
        };
        &states[match index {
            CopyIndex::A => 0,
            CopyIndex::B => 1,
        }]
    }

    fn reset_faults(&mut self) {
        self.writes = 0;
        self.syncs = 0;
        self.fail_write = None;
        self.fail_sync = None;
        self.fail_read = false;
    }

    fn crash(&mut self) {
        for state in self.journal.iter_mut().chain(self.selector.iter_mut()) {
            state.volatile = None;
        }
        self.reset_faults();
    }
}

impl RedundantRecordStorage for MemoryStorage {
    type Error = ();

    fn read_copy(
        &mut self,
        domain: RecordDomain,
        index: CopyIndex,
    ) -> Result<Option<Vec<u8>>, Self::Error> {
        if self.fail_read {
            return Err(());
        }
        Ok(self.state(domain, index).durable.clone())
    }

    fn replace_copy(
        &mut self,
        domain: RecordDomain,
        index: CopyIndex,
        bytes: &[u8],
    ) -> Result<(), Self::Error> {
        self.writes += 1;
        match self.fail_write {
            Some((nth, WriteFailure::Error)) if nth == self.writes => return Err(()),
            Some((nth, WriteFailure::Tear)) if nth == self.writes => {
                self.state_mut(domain, index).volatile = Some(bytes[..bytes.len() / 2].to_vec());
            }
            _ => self.state_mut(domain, index).volatile = Some(bytes.to_vec()),
        }
        Ok(())
    }

    fn sync_copy(&mut self, domain: RecordDomain, index: CopyIndex) -> Result<(), Self::Error> {
        self.syncs += 1;
        if self.fail_sync == Some(self.syncs) {
            return Err(());
        }
        let state = self.state_mut(domain, index);
        state.durable = state.volatile.take();
        Ok(())
    }
}

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

fn initial_journal() -> SystemJournal {
    SystemJournal {
        layout_id: LAYOUT.into(),
        floor_sequence: 5,
        confirmed: slot(SystemSlot::A, 5, 'a'),
        pending: None,
        last_rollback: None,
    }
}

fn prepared_journal() -> SystemJournal {
    let confirmed = slot(SystemSlot::A, 5, 'a');
    SystemJournal {
        layout_id: LAYOUT.into(),
        floor_sequence: 5,
        confirmed: confirmed.clone(),
        pending: Some(PendingSystemUpdate {
            operation_id: hex('1', 32),
            previous: confirmed,
            candidate: slot(SystemSlot::B, 6, 'b'),
            phase: JournalPhase::WritePrepared,
            initial_attempts: 3,
            remaining_attempts: 3,
            selector_generation: None,
            boot_id: None,
            health_binding: None,
            deadline_monotonic_ms: None,
        }),
        last_rollback: None,
    }
}

fn initial_selector() -> BootSelector {
    BootSelector {
        layout_id: LAYOUT.into(),
        confirmed_slot: SystemSlot::A,
        confirmed_manifest_sha256: hex('a', 64),
        pending: None,
    }
}

fn armed_selector() -> BootSelector {
    BootSelector {
        layout_id: LAYOUT.into(),
        confirmed_slot: SystemSlot::A,
        confirmed_manifest_sha256: hex('a', 64),
        pending: Some(PendingSelector {
            operation_id: hex('1', 32),
            previous_slot: SystemSlot::A,
            candidate_slot: SystemSlot::B,
            candidate_manifest_sha256: hex('b', 64),
            initial_attempts: 3,
            remaining_attempts: 3,
            state: SelectorPendingState::ArmedCandidate,
        }),
    }
}

fn initialized_journal_storage() -> MemoryStorage {
    let mut storage = MemoryStorage::default();
    initialize_journal_storage(&mut storage, initial_journal()).unwrap();
    storage.reset_faults();
    storage
}

#[test]
fn initialization_writes_and_verifies_both_copies() {
    let mut storage = MemoryStorage::default();
    let persisted = initialize_journal_storage(&mut storage, initial_journal()).unwrap();
    assert_eq!(persisted.generation, 1);
    let selected = load_journal_storage(&mut storage).unwrap();
    assert_eq!(selected.generation, 1);
    assert!(!selected.degraded);
    assert_eq!(selected.payload, initial_journal());
    assert_eq!(
        initialize_journal_storage(&mut storage, initial_journal()),
        Err(StorageOperationError::AlreadyInitialized)
    );
}

#[test]
fn first_copy_write_or_sync_failure_preserves_the_old_generation() {
    for (fail_write, fail_sync, expected) in [
        (
            Some((1, WriteFailure::Error)),
            None,
            StorageOperationError::Replace,
        ),
        (None, Some(1), StorageOperationError::Sync),
    ] {
        let mut storage = initialized_journal_storage();
        storage.fail_write = fail_write;
        storage.fail_sync = fail_sync;
        assert_eq!(
            persist_journal_storage(&mut storage, 1, prepared_journal()),
            Err(expected)
        );
        storage.crash();
        let selected = load_journal_storage(&mut storage).unwrap();
        assert_eq!(selected.generation, 1);
        assert!(!selected.degraded);
        assert_eq!(selected.payload, initial_journal());
    }
}

#[test]
fn torn_first_copy_is_detected_and_old_copy_remains_authoritative() {
    let mut storage = initialized_journal_storage();
    storage.fail_write = Some((1, WriteFailure::Tear));
    assert_eq!(
        persist_journal_storage(&mut storage, 1, prepared_journal()),
        Err(StorageOperationError::Verification)
    );
    storage.crash();
    let selected = load_journal_storage(&mut storage).unwrap();
    assert_eq!(selected.generation, 1);
    assert!(selected.degraded);
    assert_eq!(selected.payload, initial_journal());
}

#[test]
fn second_copy_failures_leave_the_new_generation_recoverable() {
    for (fail_write, fail_sync, expected) in [
        (
            Some((2, WriteFailure::Error)),
            None,
            StorageOperationError::Replace,
        ),
        (
            Some((2, WriteFailure::Tear)),
            None,
            StorageOperationError::Verification,
        ),
        (None, Some(2), StorageOperationError::Sync),
    ] {
        let mut storage = initialized_journal_storage();
        storage.fail_write = fail_write;
        storage.fail_sync = fail_sync;
        assert_eq!(
            persist_journal_storage(&mut storage, 1, prepared_journal()),
            Err(expected)
        );
        storage.crash();
        let selected = load_journal_storage(&mut storage).unwrap();
        assert_eq!(selected.generation, 2);
        assert!(selected.degraded);
        assert_eq!(selected.payload, prepared_journal());
    }
}

#[test]
fn successful_update_converges_both_copies_on_one_generation() {
    let mut storage = initialized_journal_storage();
    let persisted = persist_journal_storage(&mut storage, 1, prepared_journal()).unwrap();
    assert_eq!(persisted.generation, 2);
    let selected = load_journal_storage(&mut storage).unwrap();
    assert_eq!(selected.generation, 2);
    assert!(!selected.degraded);
    assert_eq!(selected.payload, prepared_journal());
}

#[test]
fn selector_storage_uses_the_same_ordering_and_fault_recovery() {
    let mut storage = MemoryStorage::default();
    initialize_selector_storage(&mut storage, initial_selector()).unwrap();
    storage.reset_faults();
    storage.fail_sync = Some(2);
    assert_eq!(
        persist_selector_storage(&mut storage, 1, armed_selector()),
        Err(StorageOperationError::Sync)
    );
    storage.crash();
    let selected = load_selector_storage(&mut storage).unwrap();
    assert_eq!(selected.generation, 2);
    assert!(selected.degraded);
    assert_eq!(selected.payload, armed_selector());
}

#[test]
fn partial_initialization_is_loadable_but_cannot_be_reinitialized() {
    let mut storage = MemoryStorage {
        fail_write: Some((2, WriteFailure::Error)),
        ..MemoryStorage::default()
    };
    assert_eq!(
        initialize_journal_storage(&mut storage, initial_journal()),
        Err(StorageOperationError::Replace)
    );
    storage.crash();
    let selected = load_journal_storage(&mut storage).unwrap();
    assert_eq!(selected.generation, 1);
    assert!(selected.degraded);
    assert_eq!(
        initialize_journal_storage(&mut storage, initial_journal()),
        Err(StorageOperationError::AlreadyInitialized)
    );
}

#[test]
fn read_failure_and_generation_exhaustion_are_explicit() {
    let mut storage = initialized_journal_storage();
    storage.fail_read = true;
    assert_eq!(
        load_journal_storage(&mut storage),
        Err(StorageOperationError::Read)
    );

    let bytes = JournalCopy::new(u64::MAX, initial_journal())
        .unwrap()
        .to_bytes()
        .unwrap();
    let mut exhausted = MemoryStorage::default();
    exhausted.journal[0].durable = Some(bytes.clone());
    exhausted.journal[1].durable = Some(bytes);
    assert_eq!(
        persist_journal_storage(&mut exhausted, u64::MAX, prepared_journal()),
        Err(StorageOperationError::GenerationExhausted)
    );
}

#[test]
fn stale_expected_generation_is_rejected_before_any_write() {
    let mut storage = initialized_journal_storage();
    assert_eq!(
        persist_journal_storage(&mut storage, 2, prepared_journal()),
        Err(StorageOperationError::ConcurrentStateChange)
    );
    assert_eq!(storage.writes, 0);
    let selected = load_journal_storage(&mut storage).unwrap();
    assert_eq!(selected.generation, 1);
    assert_eq!(selected.payload, initial_journal());
}
