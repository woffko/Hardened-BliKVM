#![no_std]
#![forbid(unsafe_code)]

//! Pure fixed-size encrypted diagnostic journal and acknowledgement planner.
//!
//! This crate performs no filesystem, process, clock, random-number, network,
//! logging, or acknowledgement I/O. Callers supply an already typed diagnostic
//! bundle, a provisioned key, and a fresh nonce. An acknowledgement action can
//! be produced only from a journal selected from authenticated redundant
//! copies; a merely prepared mutation cannot authorize it.

use core::fmt;

use bli_kvm_diagnostic_export::{DiagnosticBundle, MAX_EXPORT_BYTES, MAX_RECORDS};
use chacha20poly1305::{Key, KeyInit, Tag, XChaCha20Poly1305, XNonce, aead::AeadInOut};
use zeroize::Zeroize;

pub const DIAGNOSTIC_JOURNAL_SCHEMA_VERSION: u16 = 1;
pub const DIAGNOSTIC_JOURNAL_ALGORITHM_XCHACHA20_POLY1305: u8 = 1;
pub const DIAGNOSTIC_JOURNAL_KEY_BYTES: usize = 32;
pub const DIAGNOSTIC_JOURNAL_NONCE_BYTES: usize = 24;
pub const DIAGNOSTIC_JOURNAL_TAG_BYTES: usize = 16;
pub const MAX_DIAGNOSTIC_JOURNAL_ENTRIES: usize = 2;

pub const DIAGNOSTIC_JOURNAL_COPY_MAGIC: [u8; 8] = *b"BLKDJE01";
pub const DIAGNOSTIC_JOURNAL_PLAIN_MAGIC: [u8; 8] = *b"BLKDJPL1";
pub const DIAGNOSTIC_JOURNAL_COPY_HEADER_BYTES: usize = 64;
pub const DIAGNOSTIC_JOURNAL_PLAIN_HEADER_BYTES: usize = 32;
pub const DIAGNOSTIC_JOURNAL_ENTRY_HEADER_BYTES: usize = 28;
pub const DIAGNOSTIC_JOURNAL_ENTRY_BYTES: usize =
    DIAGNOSTIC_JOURNAL_ENTRY_HEADER_BYTES + MAX_EXPORT_BYTES;
pub const DIAGNOSTIC_JOURNAL_PLAIN_BYTES: usize = DIAGNOSTIC_JOURNAL_PLAIN_HEADER_BYTES
    + MAX_DIAGNOSTIC_JOURNAL_ENTRIES * DIAGNOSTIC_JOURNAL_ENTRY_BYTES;
pub const DIAGNOSTIC_JOURNAL_TAG_OFFSET: usize =
    DIAGNOSTIC_JOURNAL_COPY_HEADER_BYTES + DIAGNOSTIC_JOURNAL_PLAIN_BYTES;
pub const DIAGNOSTIC_JOURNAL_COPY_BYTES: usize =
    DIAGNOSTIC_JOURNAL_TAG_OFFSET + DIAGNOSTIC_JOURNAL_TAG_BYTES;

pub type RawDiagnosticJournalCopy = [u8; DIAGNOSTIC_JOURNAL_COPY_BYTES];
type RawDiagnosticJournalPlaintext = [u8; DIAGNOSTIC_JOURNAL_PLAIN_BYTES];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CopyIndex {
    A,
    B,
}

impl CopyIndex {
    pub const fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiagnosticBinding {
    instance_id: u64,
    window_id: u32,
}

impl DiagnosticBinding {
    pub const fn new(instance_id: u64, window_id: u32) -> Option<Self> {
        if instance_id == 0 || window_id == 0 {
            None
        } else {
            Some(Self {
                instance_id,
                window_id,
            })
        }
    }

    pub const fn instance_id(self) -> u64 {
        self.instance_id
    }

    pub const fn window_id(self) -> u32 {
        self.window_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticJournalEntryState {
    PendingAcknowledgement,
    AcknowledgementAmbiguous,
    Acknowledged,
    Orphaned,
}

impl DiagnosticJournalEntryState {
    const fn is_nonterminal(self) -> bool {
        matches!(
            self,
            Self::PendingAcknowledgement | Self::AcknowledgementAmbiguous
        )
    }

    const fn tag(self) -> u8 {
        match self {
            Self::PendingAcknowledgement => 1,
            Self::AcknowledgementAmbiguous => 2,
            Self::Acknowledged => 3,
            Self::Orphaned => 4,
        }
    }

    const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::PendingAcknowledgement),
            2 => Some(Self::AcknowledgementAmbiguous),
            3 => Some(Self::Acknowledged),
            4 => Some(Self::Orphaned),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticJournalError {
    InvalidKey,
    DuplicateKeyId,
    InvalidNonce,
    NonceReuse,
    InvalidLength,
    InvalidMagic,
    UnsupportedSchema,
    UnsupportedAlgorithm,
    InvalidReserved,
    InvalidEncoding,
    InvalidGeneration,
    GenerationExhausted,
    EntryIdExhausted,
    InvalidBinding,
    InvalidBundle,
    DuplicateBinding,
    MultiplePendingEntries,
    RetentionBlocked,
    SnapshotConflict,
    StateConflict,
    KeyUnavailable,
    AuthenticationFailed,
    GenerationMismatch,
    GenerationGap,
    SplitBrain,
    NoValidCopy,
    RollbackDetected,
    ConcurrentStateChange,
    VerificationFailed,
}

/// Owned key material. It intentionally implements neither `Clone` nor `Debug`.
pub struct DiagnosticJournalKey {
    key_id: u32,
    bytes: [u8; DIAGNOSTIC_JOURNAL_KEY_BYTES],
}

impl DiagnosticJournalKey {
    pub fn new(
        key_id: u32,
        mut bytes: [u8; DIAGNOSTIC_JOURNAL_KEY_BYTES],
    ) -> Result<Self, DiagnosticJournalError> {
        if key_id == 0 || bytes.iter().all(|byte| *byte == 0) {
            bytes.zeroize();
            return Err(DiagnosticJournalError::InvalidKey);
        }
        Ok(Self { key_id, bytes })
    }

    pub const fn key_id(&self) -> u32 {
        self.key_id
    }
}

impl Drop for DiagnosticJournalKey {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

/// Active plus optional previous key used during bounded rotation.
///
/// It intentionally implements neither `Clone` nor `Debug`.
pub struct DiagnosticJournalKeyRing<'a> {
    active: &'a DiagnosticJournalKey,
    previous: Option<&'a DiagnosticJournalKey>,
}

impl<'a> DiagnosticJournalKeyRing<'a> {
    pub fn new(
        active: &'a DiagnosticJournalKey,
        previous: Option<&'a DiagnosticJournalKey>,
    ) -> Result<Self, DiagnosticJournalError> {
        if previous.is_some_and(|previous| previous.key_id == active.key_id) {
            return Err(DiagnosticJournalError::DuplicateKeyId);
        }
        Ok(Self { active, previous })
    }

    pub const fn active(&self) -> &DiagnosticJournalKey {
        self.active
    }

    fn key_for(&self, key_id: u32) -> Option<&DiagnosticJournalKey> {
        if self.active.key_id == key_id {
            Some(self.active)
        } else {
            self.previous.filter(|key| key.key_id == key_id)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DiagnosticJournalNonce {
    bytes: [u8; DIAGNOSTIC_JOURNAL_NONCE_BYTES],
}

impl DiagnosticJournalNonce {
    pub const fn new(bytes: [u8; DIAGNOSTIC_JOURNAL_NONCE_BYTES]) -> Option<Self> {
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] != 0 {
                return Some(Self { bytes });
            }
            index += 1;
        }
        None
    }

    pub const fn as_bytes(&self) -> &[u8; DIAGNOSTIC_JOURNAL_NONCE_BYTES] {
        &self.bytes
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DiagnosticJournalEntry {
    entry_id: u64,
    binding: DiagnosticBinding,
    omitted_lifecycle_events: u8,
    state: DiagnosticJournalEntryState,
    bundle: DiagnosticBundle,
}

impl DiagnosticJournalEntry {
    pub const fn entry_id(&self) -> u64 {
        self.entry_id
    }

    pub const fn binding(&self) -> DiagnosticBinding {
        self.binding
    }

    pub const fn omitted_lifecycle_events(&self) -> u8 {
        self.omitted_lifecycle_events
    }

    pub const fn state(&self) -> DiagnosticJournalEntryState {
        self.state
    }

    pub const fn bundle(&self) -> &DiagnosticBundle {
        &self.bundle
    }

    fn matches_snapshot(&self, snapshot: DiagnosticSnapshot<'_>) -> bool {
        self.binding == snapshot.binding
            && self.omitted_lifecycle_events == snapshot.omitted_lifecycle_events
            && self.bundle == *snapshot.bundle
    }
}

impl fmt::Debug for DiagnosticJournalEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiagnosticJournalEntry")
            .field("entry_id", &self.entry_id)
            .field("binding", &self.binding)
            .field("omitted_lifecycle_events", &self.omitted_lifecycle_events)
            .field("state", &self.state)
            .field("record_count", &self.bundle.record_count())
            .field("bundle_length", &self.bundle.length())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DiagnosticJournal {
    generation: u64,
    next_entry_id: u64,
    entries: [Option<DiagnosticJournalEntry>; MAX_DIAGNOSTIC_JOURNAL_ENTRIES],
}

impl DiagnosticJournal {
    pub fn empty() -> Self {
        Self {
            generation: 0,
            next_entry_id: 1,
            entries: core::array::from_fn(|_| None),
        }
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn next_entry_id(&self) -> u64 {
        self.next_entry_id
    }

    pub fn entry_count(&self) -> usize {
        self.entries.iter().flatten().count()
    }

    pub fn entries(&self) -> impl Iterator<Item = &DiagnosticJournalEntry> {
        self.entries.iter().flatten()
    }

    pub fn entry(&self, binding: DiagnosticBinding) -> Option<&DiagnosticJournalEntry> {
        self.entries().find(|entry| entry.binding == binding)
    }

    fn validate(&self) -> Result<(), DiagnosticJournalError> {
        if self.next_entry_id == 0
            || self.generation == 0 && (self.entry_count() != 0 || self.next_entry_id != 1)
        {
            return Err(DiagnosticJournalError::InvalidGeneration);
        }
        let mut empty_seen = false;
        let mut previous_id = 0_u64;
        let mut pending = 0_usize;
        for (index, slot) in self.entries.iter().enumerate() {
            let Some(entry) = slot else {
                empty_seen = true;
                continue;
            };
            if empty_seen || entry.entry_id == 0 || entry.entry_id <= previous_id {
                return Err(DiagnosticJournalError::InvalidEncoding);
            }
            previous_id = entry.entry_id;
            if entry.binding.instance_id == 0
                || entry.binding.window_id == 0
                || entry.bundle.record_count() == 0
                || entry.bundle.record_count() > MAX_RECORDS
            {
                return Err(DiagnosticJournalError::InvalidBundle);
            }
            if entry.state.is_nonterminal() {
                pending += 1;
            }
            for other in self.entries.iter().take(index).flatten() {
                if other.binding == entry.binding {
                    return Err(DiagnosticJournalError::DuplicateBinding);
                }
            }
        }
        if pending > 1 {
            return Err(DiagnosticJournalError::MultiplePendingEntries);
        }
        if previous_id >= self.next_entry_id {
            return Err(DiagnosticJournalError::InvalidEncoding);
        }
        Ok(())
    }

    fn bumped(&self) -> Result<Self, DiagnosticJournalError> {
        let mut next = self.clone();
        next.generation = self
            .generation
            .checked_add(1)
            .ok_or(DiagnosticJournalError::GenerationExhausted)?;
        Ok(next)
    }

    fn active_index(&self) -> Option<usize> {
        self.entries.iter().position(|entry| {
            entry
                .as_ref()
                .is_some_and(|entry| entry.state.is_nonterminal())
        })
    }

    fn binding_index(&self, binding: DiagnosticBinding) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.as_ref().is_some_and(|entry| entry.binding == binding))
    }

    fn plan_state(
        &self,
        index: usize,
        state: DiagnosticJournalEntryState,
    ) -> Result<JournalMutation, DiagnosticJournalError> {
        let mut next = self.bumped()?;
        next.entries
            .get_mut(index)
            .and_then(Option::as_mut)
            .ok_or(DiagnosticJournalError::StateConflict)?
            .state = state;
        next.validate()?;
        Ok(JournalMutation {
            expected_generation: self.generation,
            journal: next,
        })
    }

    fn plan_append(
        &self,
        snapshot: DiagnosticSnapshot<'_>,
    ) -> Result<JournalMutation, DiagnosticJournalError> {
        if self.active_index().is_some() {
            return Err(DiagnosticJournalError::StateConflict);
        }
        let mut next = self.bumped()?;
        let mut count = next.entry_count();
        if count == MAX_DIAGNOSTIC_JOURNAL_ENTRIES {
            let evictable = next.entries[0]
                .as_ref()
                .is_some_and(|entry| !entry.state.is_nonterminal());
            if !evictable {
                return Err(DiagnosticJournalError::RetentionBlocked);
            }
            for index in 1..MAX_DIAGNOSTIC_JOURNAL_ENTRIES {
                next.entries[index - 1] = next.entries[index].take();
            }
            next.entries[MAX_DIAGNOSTIC_JOURNAL_ENTRIES - 1] = None;
            count -= 1;
        }
        let entry_id = next.next_entry_id;
        next.next_entry_id = entry_id
            .checked_add(1)
            .ok_or(DiagnosticJournalError::EntryIdExhausted)?;
        next.entries[count] = Some(DiagnosticJournalEntry {
            entry_id,
            binding: snapshot.binding,
            omitted_lifecycle_events: snapshot.omitted_lifecycle_events,
            state: DiagnosticJournalEntryState::PendingAcknowledgement,
            bundle: snapshot.bundle.clone(),
        });
        next.validate()?;
        Ok(JournalMutation {
            expected_generation: self.generation,
            journal: next,
        })
    }
}

impl Default for DiagnosticJournal {
    fn default() -> Self {
        Self::empty()
    }
}

impl fmt::Debug for DiagnosticJournal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiagnosticJournal")
            .field("generation", &self.generation)
            .field("next_entry_id", &self.next_entry_id)
            .field("entry_count", &self.entry_count())
            .field("entries", &self.entries)
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticSnapshot<'a> {
    binding: DiagnosticBinding,
    omitted_lifecycle_events: u8,
    bundle: &'a DiagnosticBundle,
}

impl fmt::Debug for DiagnosticSnapshot<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiagnosticSnapshot")
            .field("binding", &self.binding)
            .field("omitted_lifecycle_events", &self.omitted_lifecycle_events)
            .field("record_count", &self.bundle.record_count())
            .field("bundle_length", &self.bundle.length())
            .finish_non_exhaustive()
    }
}

impl<'a> DiagnosticSnapshot<'a> {
    pub fn new(
        binding: DiagnosticBinding,
        omitted_lifecycle_events: u8,
        bundle: &'a DiagnosticBundle,
    ) -> Result<Self, DiagnosticJournalError> {
        if bundle.record_count() == 0
            || bundle.record_count() > MAX_RECORDS
            || DiagnosticBundle::decode(bundle.as_bytes()).is_err()
        {
            return Err(DiagnosticJournalError::InvalidBundle);
        }
        Ok(Self {
            binding,
            omitted_lifecycle_events,
            bundle,
        })
    }

    pub const fn binding(self) -> DiagnosticBinding {
        self.binding
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectorObservation<'a> {
    NoSealed,
    Sealed(DiagnosticSnapshot<'a>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcknowledgementResult {
    Applied,
    NoSealed,
    Mismatch,
    DeliveryAmbiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconciliationAction {
    Idle,
    Persist,
    Acknowledge(DiagnosticBinding),
    ReadCollector,
}

pub struct ReconciliationPlan {
    action: ReconciliationAction,
    mutation: Option<JournalMutation>,
}

impl ReconciliationPlan {
    pub const fn action(&self) -> ReconciliationAction {
        self.action
    }

    pub const fn mutation(&self) -> Option<&JournalMutation> {
        self.mutation.as_ref()
    }

    fn without_mutation(action: ReconciliationAction) -> Self {
        Self {
            action,
            mutation: None,
        }
    }

    fn persist(mutation: JournalMutation) -> Self {
        Self {
            action: ReconciliationAction::Persist,
            mutation: Some(mutation),
        }
    }
}

impl fmt::Debug for ReconciliationPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReconciliationPlan")
            .field("action", &self.action)
            .field(
                "mutation_generation",
                &self.mutation.as_ref().map(|mutation| mutation.generation()),
            )
            .finish()
    }
}

pub struct JournalMutation {
    expected_generation: u64,
    journal: DiagnosticJournal,
}

impl JournalMutation {
    pub const fn expected_generation(&self) -> u64 {
        self.expected_generation
    }

    pub const fn generation(&self) -> u64 {
        self.journal.generation
    }

    pub const fn journal(&self) -> &DiagnosticJournal {
        &self.journal
    }
}

impl fmt::Debug for JournalMutation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JournalMutation")
            .field("expected_generation", &self.expected_generation)
            .field("generation", &self.journal.generation)
            .field("entry_count", &self.journal.entry_count())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CopyMetadata {
    key_id: u32,
    nonce: DiagnosticJournalNonce,
}

struct OpenedCopy {
    raw: RawDiagnosticJournalCopy,
    journal: DiagnosticJournal,
    metadata: CopyMetadata,
}

pub struct SelectedDiagnosticJournal {
    selected_index: CopyIndex,
    journal: DiagnosticJournal,
    degraded: bool,
    repair_target: Option<CopyIndex>,
    selected_bytes: RawDiagnosticJournalCopy,
    metadata: [Option<CopyMetadata>; 2],
}

impl SelectedDiagnosticJournal {
    pub const fn selected_index(&self) -> CopyIndex {
        self.selected_index
    }

    pub const fn generation(&self) -> u64 {
        self.journal.generation
    }

    pub const fn journal(&self) -> &DiagnosticJournal {
        &self.journal
    }

    pub const fn degraded(&self) -> bool {
        self.degraded
    }

    pub const fn repair_target(&self) -> Option<CopyIndex> {
        self.repair_target
    }

    fn nonce_used(&self, key_id: u32, nonce: DiagnosticJournalNonce) -> bool {
        self.metadata
            .iter()
            .flatten()
            .any(|metadata| metadata.key_id == key_id && metadata.nonce == nonce)
    }
}

impl fmt::Debug for SelectedDiagnosticJournal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SelectedDiagnosticJournal")
            .field("selected_index", &self.selected_index)
            .field("generation", &self.journal.generation)
            .field("degraded", &self.degraded)
            .field("repair_target", &self.repair_target)
            .field("entry_count", &self.journal.entry_count())
            .finish_non_exhaustive()
    }
}

pub struct JournalWritePlan {
    expected_generation: u64,
    generation: u64,
    first: CopyIndex,
    second: CopyIndex,
    bytes: RawDiagnosticJournalCopy,
}

impl JournalWritePlan {
    pub const fn expected_generation(&self) -> u64 {
        self.expected_generation
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn first(&self) -> CopyIndex {
        self.first
    }

    pub const fn second(&self) -> CopyIndex {
        self.second
    }

    pub const fn bytes(&self) -> &RawDiagnosticJournalCopy {
        &self.bytes
    }
}

impl fmt::Debug for JournalWritePlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JournalWritePlan")
            .field("expected_generation", &self.expected_generation)
            .field("generation", &self.generation)
            .field("first", &self.first)
            .field("second", &self.second)
            .finish_non_exhaustive()
    }
}

pub struct JournalRepairPlan {
    expected_generation: u64,
    target: CopyIndex,
    bytes: RawDiagnosticJournalCopy,
}

impl JournalRepairPlan {
    pub const fn expected_generation(&self) -> u64 {
        self.expected_generation
    }

    pub const fn target(&self) -> CopyIndex {
        self.target
    }

    pub const fn bytes(&self) -> &RawDiagnosticJournalCopy {
        &self.bytes
    }
}

impl fmt::Debug for JournalRepairPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JournalRepairPlan")
            .field("expected_generation", &self.expected_generation)
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

pub fn plan_initialize(
    journal: &DiagnosticJournal,
    keys: &DiagnosticJournalKeyRing<'_>,
    nonce: DiagnosticJournalNonce,
) -> Result<JournalWritePlan, DiagnosticJournalError> {
    journal.validate()?;
    if journal.generation != 0 || journal.entry_count() != 0 || journal.next_entry_id != 1 {
        return Err(DiagnosticJournalError::StateConflict);
    }
    let mut initialized = journal.clone();
    initialized.generation = 1;
    let bytes = seal_copy(&initialized, keys.active(), nonce)?;
    Ok(JournalWritePlan {
        expected_generation: 0,
        generation: 1,
        first: CopyIndex::A,
        second: CopyIndex::B,
        bytes,
    })
}

pub fn plan_update(
    selected: &SelectedDiagnosticJournal,
    mutation: &JournalMutation,
    keys: &DiagnosticJournalKeyRing<'_>,
    nonce: DiagnosticJournalNonce,
) -> Result<JournalWritePlan, DiagnosticJournalError> {
    if mutation.expected_generation != selected.generation() {
        return Err(DiagnosticJournalError::ConcurrentStateChange);
    }
    if mutation.journal.generation
        != selected
            .generation()
            .checked_add(1)
            .ok_or(DiagnosticJournalError::GenerationExhausted)?
    {
        return Err(DiagnosticJournalError::GenerationMismatch);
    }
    mutation.journal.validate()?;
    if selected.nonce_used(keys.active().key_id, nonce) {
        return Err(DiagnosticJournalError::NonceReuse);
    }
    let bytes = seal_copy(&mutation.journal, keys.active(), nonce)?;
    Ok(JournalWritePlan {
        expected_generation: selected.generation(),
        generation: mutation.journal.generation,
        first: selected.selected_index.other(),
        second: selected.selected_index,
        bytes,
    })
}

pub fn plan_repair(selected: &SelectedDiagnosticJournal) -> Option<JournalRepairPlan> {
    selected.repair_target.map(|target| JournalRepairPlan {
        expected_generation: selected.generation(),
        target,
        bytes: selected.selected_bytes,
    })
}

pub fn verify_written(
    expected: &RawDiagnosticJournalCopy,
    observed: &[u8],
    keys: &DiagnosticJournalKeyRing<'_>,
) -> Result<(), DiagnosticJournalError> {
    if expected.as_slice() != observed {
        return Err(DiagnosticJournalError::VerificationFailed);
    }
    let opened = open_copy(observed, keys)?;
    if opened.raw != *expected {
        return Err(DiagnosticJournalError::VerificationFailed);
    }
    Ok(())
}

pub fn select_copies(
    copy_a: Option<&[u8]>,
    copy_b: Option<&[u8]>,
    keys: &DiagnosticJournalKeyRing<'_>,
    minimum_generation: u64,
) -> Result<SelectedDiagnosticJournal, DiagnosticJournalError> {
    let a = decode_candidate(copy_a, keys)?;
    let b = decode_candidate(copy_b, keys)?;
    let metadata = [
        a.as_ref().map(|copy| copy.metadata),
        b.as_ref().map(|copy| copy.metadata),
    ];
    let (selected_index, selected, degraded, repair_target) = match (a, b) {
        (None, None) => return Err(DiagnosticJournalError::NoValidCopy),
        (Some(copy), None) => (CopyIndex::A, copy, true, Some(CopyIndex::B)),
        (None, Some(copy)) => (CopyIndex::B, copy, true, Some(CopyIndex::A)),
        (Some(a), Some(b)) if a.journal.generation == b.journal.generation => {
            if a.raw != b.raw || a.journal != b.journal {
                return Err(DiagnosticJournalError::SplitBrain);
            }
            (CopyIndex::A, a, false, None)
        }
        (Some(a), Some(b)) => {
            if a.journal.generation.abs_diff(b.journal.generation) != 1 {
                return Err(DiagnosticJournalError::GenerationGap);
            }
            if a.journal.generation > b.journal.generation {
                (CopyIndex::A, a, true, Some(CopyIndex::B))
            } else {
                (CopyIndex::B, b, true, Some(CopyIndex::A))
            }
        }
    };
    if selected.journal.generation < minimum_generation {
        return Err(DiagnosticJournalError::RollbackDetected);
    }
    Ok(SelectedDiagnosticJournal {
        selected_index,
        journal: selected.journal,
        degraded,
        repair_target,
        selected_bytes: selected.raw,
        metadata,
    })
}

fn decode_candidate(
    bytes: Option<&[u8]>,
    keys: &DiagnosticJournalKeyRing<'_>,
) -> Result<Option<OpenedCopy>, DiagnosticJournalError> {
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    match open_copy(bytes, keys) {
        Ok(copy) => Ok(Some(copy)),
        Err(
            error @ (DiagnosticJournalError::KeyUnavailable
            | DiagnosticJournalError::UnsupportedSchema
            | DiagnosticJournalError::UnsupportedAlgorithm),
        ) => Err(error),
        Err(_) => Ok(None),
    }
}

pub fn plan_reconcile(
    selected: &SelectedDiagnosticJournal,
    observation: CollectorObservation<'_>,
) -> Result<ReconciliationPlan, DiagnosticJournalError> {
    let journal = &selected.journal;
    let active = journal.active_index();
    match observation {
        CollectorObservation::NoSealed => match active {
            Some(index) => Ok(ReconciliationPlan::persist(
                journal.plan_state(index, DiagnosticJournalEntryState::Orphaned)?,
            )),
            None => Ok(ReconciliationPlan::without_mutation(
                ReconciliationAction::Idle,
            )),
        },
        CollectorObservation::Sealed(snapshot) => {
            if let Some(index) = active {
                let entry = journal.entries[index]
                    .as_ref()
                    .ok_or(DiagnosticJournalError::StateConflict)?;
                if entry.binding != snapshot.binding {
                    return Ok(ReconciliationPlan::persist(
                        journal.plan_state(index, DiagnosticJournalEntryState::Orphaned)?,
                    ));
                }
                if !entry.matches_snapshot(snapshot) {
                    return Err(DiagnosticJournalError::SnapshotConflict);
                }
                return match entry.state {
                    DiagnosticJournalEntryState::PendingAcknowledgement => {
                        Ok(ReconciliationPlan::without_mutation(
                            ReconciliationAction::Acknowledge(entry.binding),
                        ))
                    }
                    DiagnosticJournalEntryState::AcknowledgementAmbiguous => {
                        Ok(ReconciliationPlan::persist(journal.plan_state(
                            index,
                            DiagnosticJournalEntryState::PendingAcknowledgement,
                        )?))
                    }
                    DiagnosticJournalEntryState::Acknowledged
                    | DiagnosticJournalEntryState::Orphaned => {
                        Err(DiagnosticJournalError::StateConflict)
                    }
                };
            }
            if let Some(index) = journal.binding_index(snapshot.binding) {
                let entry = journal.entries[index]
                    .as_ref()
                    .ok_or(DiagnosticJournalError::StateConflict)?;
                if !entry.matches_snapshot(snapshot) {
                    return Err(DiagnosticJournalError::SnapshotConflict);
                }
                return Err(DiagnosticJournalError::StateConflict);
            }
            Ok(ReconciliationPlan::persist(journal.plan_append(snapshot)?))
        }
    }
}

pub fn plan_acknowledgement_result(
    selected: &SelectedDiagnosticJournal,
    binding: DiagnosticBinding,
    result: AcknowledgementResult,
) -> Result<ReconciliationPlan, DiagnosticJournalError> {
    let journal = &selected.journal;
    let index = journal
        .active_index()
        .ok_or(DiagnosticJournalError::StateConflict)?;
    let entry = journal.entries[index]
        .as_ref()
        .ok_or(DiagnosticJournalError::StateConflict)?;
    if entry.binding != binding
        || entry.state != DiagnosticJournalEntryState::PendingAcknowledgement
    {
        return Err(DiagnosticJournalError::StateConflict);
    }
    match result {
        AcknowledgementResult::Applied => Ok(ReconciliationPlan::persist(
            journal.plan_state(index, DiagnosticJournalEntryState::Acknowledged)?,
        )),
        AcknowledgementResult::NoSealed => Ok(ReconciliationPlan::persist(
            journal.plan_state(index, DiagnosticJournalEntryState::Orphaned)?,
        )),
        AcknowledgementResult::Mismatch => Ok(ReconciliationPlan::without_mutation(
            ReconciliationAction::ReadCollector,
        )),
        AcknowledgementResult::DeliveryAmbiguous => Ok(ReconciliationPlan::persist(
            journal.plan_state(index, DiagnosticJournalEntryState::AcknowledgementAmbiguous)?,
        )),
    }
}

fn seal_copy(
    journal: &DiagnosticJournal,
    key: &DiagnosticJournalKey,
    nonce: DiagnosticJournalNonce,
) -> Result<RawDiagnosticJournalCopy, DiagnosticJournalError> {
    journal.validate()?;
    if journal.generation == 0 {
        return Err(DiagnosticJournalError::InvalidGeneration);
    }
    let mut plaintext = encode_plaintext(journal)?;
    let mut bytes = [0_u8; DIAGNOSTIC_JOURNAL_COPY_BYTES];
    if let Err(error) = encode_copy_header(
        &mut bytes[..DIAGNOSTIC_JOURNAL_COPY_HEADER_BYTES],
        key,
        nonce,
        journal,
    ) {
        plaintext.zeroize();
        return Err(error);
    }
    bytes[DIAGNOSTIC_JOURNAL_COPY_HEADER_BYTES..DIAGNOSTIC_JOURNAL_TAG_OFFSET]
        .copy_from_slice(&plaintext);
    plaintext.zeroize();

    let mut aead_key = Key::from(key.bytes);
    let cipher = XChaCha20Poly1305::new(&aead_key);
    aead_key.zeroize();
    let xnonce = XNonce::from(nonce.bytes);
    let (header, body) = bytes.split_at_mut(DIAGNOSTIC_JOURNAL_COPY_HEADER_BYTES);
    let (ciphertext, tag_bytes) = body.split_at_mut(DIAGNOSTIC_JOURNAL_PLAIN_BYTES);
    let tag = match cipher.encrypt_inout_detached(&xnonce, header, ciphertext.into()) {
        Ok(tag) => tag,
        Err(_) => {
            ciphertext.zeroize();
            return Err(DiagnosticJournalError::AuthenticationFailed);
        }
    };
    tag_bytes.copy_from_slice(&tag);
    Ok(bytes)
}

fn open_copy(
    bytes: &[u8],
    keys: &DiagnosticJournalKeyRing<'_>,
) -> Result<OpenedCopy, DiagnosticJournalError> {
    if bytes.len() != DIAGNOSTIC_JOURNAL_COPY_BYTES {
        return Err(DiagnosticJournalError::InvalidLength);
    }
    let original: RawDiagnosticJournalCopy = bytes
        .try_into()
        .map_err(|_| DiagnosticJournalError::InvalidLength)?;
    let mut raw = original;
    let (key_id, generation, nonce) = decode_copy_header(&raw)?;
    let key = keys
        .key_for(key_id)
        .ok_or(DiagnosticJournalError::KeyUnavailable)?;
    let mut aead_key = Key::from(key.bytes);
    let cipher = XChaCha20Poly1305::new(&aead_key);
    aead_key.zeroize();
    let xnonce = XNonce::from(nonce.bytes);
    let (header, body) = raw.split_at_mut(DIAGNOSTIC_JOURNAL_COPY_HEADER_BYTES);
    let (ciphertext, tag_bytes) = body.split_at_mut(DIAGNOSTIC_JOURNAL_PLAIN_BYTES);
    let tag = Tag::try_from(&*tag_bytes).map_err(|_| DiagnosticJournalError::InvalidLength)?;
    if cipher
        .decrypt_inout_detached(&xnonce, header, ciphertext.into(), &tag)
        .is_err()
    {
        ciphertext.zeroize();
        return Err(DiagnosticJournalError::AuthenticationFailed);
    }
    let decoded = decode_plaintext(ciphertext);
    ciphertext.zeroize();
    let journal = decoded?;
    if journal.generation != generation {
        return Err(DiagnosticJournalError::GenerationMismatch);
    }
    // Re-sealing is intentionally not used as a canonicality test because the
    // ciphertext itself is already authenticated. Plaintext canonicality was
    // checked by decode_plaintext/encode_plaintext.
    Ok(OpenedCopy {
        raw: original,
        journal,
        metadata: CopyMetadata { key_id, nonce },
    })
}

fn encode_copy_header(
    header: &mut [u8],
    key: &DiagnosticJournalKey,
    nonce: DiagnosticJournalNonce,
    journal: &DiagnosticJournal,
) -> Result<(), DiagnosticJournalError> {
    if header.len() != DIAGNOSTIC_JOURNAL_COPY_HEADER_BYTES {
        return Err(DiagnosticJournalError::InvalidLength);
    }
    header.fill(0);
    header[..8].copy_from_slice(&DIAGNOSTIC_JOURNAL_COPY_MAGIC);
    header[8..10].copy_from_slice(&DIAGNOSTIC_JOURNAL_SCHEMA_VERSION.to_be_bytes());
    header[10] = DIAGNOSTIC_JOURNAL_ALGORITHM_XCHACHA20_POLY1305;
    header[12..16].copy_from_slice(&key.key_id.to_be_bytes());
    header[16..24].copy_from_slice(&journal.generation.to_be_bytes());
    header[24..48].copy_from_slice(&nonce.bytes);
    header[48..52].copy_from_slice(&(DIAGNOSTIC_JOURNAL_PLAIN_BYTES as u32).to_be_bytes());
    Ok(())
}

fn decode_copy_header(
    bytes: &[u8; DIAGNOSTIC_JOURNAL_COPY_BYTES],
) -> Result<(u32, u64, DiagnosticJournalNonce), DiagnosticJournalError> {
    let header = &bytes[..DIAGNOSTIC_JOURNAL_COPY_HEADER_BYTES];
    if header[..8] != DIAGNOSTIC_JOURNAL_COPY_MAGIC {
        return Err(DiagnosticJournalError::InvalidMagic);
    }
    if u16::from_be_bytes(
        header[8..10]
            .try_into()
            .map_err(|_| DiagnosticJournalError::InvalidLength)?,
    ) != DIAGNOSTIC_JOURNAL_SCHEMA_VERSION
    {
        return Err(DiagnosticJournalError::UnsupportedSchema);
    }
    if header[10] != DIAGNOSTIC_JOURNAL_ALGORITHM_XCHACHA20_POLY1305 {
        return Err(DiagnosticJournalError::UnsupportedAlgorithm);
    }
    if header[11] != 0 || header[52..].iter().any(|byte| *byte != 0) {
        return Err(DiagnosticJournalError::InvalidReserved);
    }
    let key_id = u32::from_be_bytes(
        header[12..16]
            .try_into()
            .map_err(|_| DiagnosticJournalError::InvalidLength)?,
    );
    let generation = u64::from_be_bytes(
        header[16..24]
            .try_into()
            .map_err(|_| DiagnosticJournalError::InvalidLength)?,
    );
    let nonce = DiagnosticJournalNonce::new(
        header[24..48]
            .try_into()
            .map_err(|_| DiagnosticJournalError::InvalidLength)?,
    )
    .ok_or(DiagnosticJournalError::InvalidNonce)?;
    let ciphertext_length = u32::from_be_bytes(
        header[48..52]
            .try_into()
            .map_err(|_| DiagnosticJournalError::InvalidLength)?,
    );
    if key_id == 0 || generation == 0 {
        return Err(DiagnosticJournalError::InvalidGeneration);
    }
    if ciphertext_length as usize != DIAGNOSTIC_JOURNAL_PLAIN_BYTES {
        return Err(DiagnosticJournalError::InvalidLength);
    }
    Ok((key_id, generation, nonce))
}

fn encode_plaintext(
    journal: &DiagnosticJournal,
) -> Result<RawDiagnosticJournalPlaintext, DiagnosticJournalError> {
    journal.validate()?;
    let mut writer = FixedWriter::<DIAGNOSTIC_JOURNAL_PLAIN_BYTES>::new();
    writer.write_slice(&DIAGNOSTIC_JOURNAL_PLAIN_MAGIC)?;
    writer.write_u16(DIAGNOSTIC_JOURNAL_SCHEMA_VERSION)?;
    writer.write_u8(
        u8::try_from(journal.entry_count()).map_err(|_| DiagnosticJournalError::InvalidEncoding)?,
    )?;
    writer.skip(5)?;
    writer.write_u64(journal.generation)?;
    writer.write_u64(journal.next_entry_id)?;
    for slot in &journal.entries {
        if let Some(entry) = slot {
            writer.write_u8(entry.state.tag())?;
            writer.skip(3)?;
            writer.write_u64(entry.entry_id)?;
            writer.write_u64(entry.binding.instance_id)?;
            writer.write_u32(entry.binding.window_id)?;
            writer.write_u8(entry.omitted_lifecycle_events)?;
            writer.write_u8(
                u8::try_from(entry.bundle.record_count())
                    .map_err(|_| DiagnosticJournalError::InvalidBundle)?,
            )?;
            writer.write_u16(
                u16::try_from(entry.bundle.length())
                    .map_err(|_| DiagnosticJournalError::InvalidBundle)?,
            )?;
            writer.write_slice(entry.bundle.as_bytes())?;
            writer.skip(MAX_EXPORT_BYTES - entry.bundle.length())?;
        } else {
            writer.skip(DIAGNOSTIC_JOURNAL_ENTRY_BYTES)?;
        }
    }
    writer.finish()
}

fn decode_plaintext(bytes: &[u8]) -> Result<DiagnosticJournal, DiagnosticJournalError> {
    if bytes.len() != DIAGNOSTIC_JOURNAL_PLAIN_BYTES {
        return Err(DiagnosticJournalError::InvalidLength);
    }
    let mut reader = FixedReader::new(bytes);
    if reader.read_array::<8>()? != DIAGNOSTIC_JOURNAL_PLAIN_MAGIC {
        return Err(DiagnosticJournalError::InvalidMagic);
    }
    if reader.read_u16()? != DIAGNOSTIC_JOURNAL_SCHEMA_VERSION {
        return Err(DiagnosticJournalError::UnsupportedSchema);
    }
    let entry_count = usize::from(reader.read_u8()?);
    if entry_count > MAX_DIAGNOSTIC_JOURNAL_ENTRIES
        || reader.read_slice(5)?.iter().any(|byte| *byte != 0)
    {
        return Err(DiagnosticJournalError::InvalidEncoding);
    }
    let generation = reader.read_u64()?;
    let next_entry_id = reader.read_u64()?;
    let mut entries = core::array::from_fn(|_| None);
    for (index, slot) in entries.iter_mut().enumerate() {
        let raw = reader.read_slice(DIAGNOSTIC_JOURNAL_ENTRY_BYTES)?;
        if index >= entry_count {
            if raw.iter().any(|byte| *byte != 0) {
                return Err(DiagnosticJournalError::InvalidEncoding);
            }
            continue;
        }
        let mut entry_reader = FixedReader::new(raw);
        let state = DiagnosticJournalEntryState::from_tag(entry_reader.read_u8()?)
            .ok_or(DiagnosticJournalError::InvalidEncoding)?;
        if entry_reader.read_slice(3)?.iter().any(|byte| *byte != 0) {
            return Err(DiagnosticJournalError::InvalidReserved);
        }
        let entry_id = entry_reader.read_u64()?;
        let binding = DiagnosticBinding::new(entry_reader.read_u64()?, entry_reader.read_u32()?)
            .ok_or(DiagnosticJournalError::InvalidBinding)?;
        let omitted_lifecycle_events = entry_reader.read_u8()?;
        let record_count = entry_reader.read_u8()?;
        let bundle_length = usize::from(entry_reader.read_u16()?);
        if record_count == 0
            || usize::from(record_count) > MAX_RECORDS
            || bundle_length == 0
            || bundle_length > MAX_EXPORT_BYTES
        {
            return Err(DiagnosticJournalError::InvalidBundle);
        }
        let bundle_bytes = entry_reader.read_slice(bundle_length)?;
        if entry_reader
            .read_slice(MAX_EXPORT_BYTES - bundle_length)?
            .iter()
            .any(|byte| *byte != 0)
            || !entry_reader.is_finished()
        {
            return Err(DiagnosticJournalError::InvalidEncoding);
        }
        let bundle = DiagnosticBundle::decode(bundle_bytes)
            .map_err(|_| DiagnosticJournalError::InvalidBundle)?;
        if bundle.record_count() != usize::from(record_count) {
            return Err(DiagnosticJournalError::InvalidBundle);
        }
        *slot = Some(DiagnosticJournalEntry {
            entry_id,
            binding,
            omitted_lifecycle_events,
            state,
            bundle,
        });
    }
    if !reader.is_finished() {
        return Err(DiagnosticJournalError::InvalidLength);
    }
    let journal = DiagnosticJournal {
        generation,
        next_entry_id,
        entries,
    };
    journal.validate()?;
    let mut canonical = encode_plaintext(&journal)?;
    let is_canonical = canonical.as_slice() == bytes;
    canonical.zeroize();
    if !is_canonical {
        return Err(DiagnosticJournalError::InvalidEncoding);
    }
    Ok(journal)
}

struct FixedWriter<const N: usize> {
    bytes: [u8; N],
    cursor: usize,
}

impl<const N: usize> FixedWriter<N> {
    const fn new() -> Self {
        Self {
            bytes: [0; N],
            cursor: 0,
        }
    }

    fn write_u8(&mut self, value: u8) -> Result<(), DiagnosticJournalError> {
        self.write_slice(&[value])
    }

    fn write_u16(&mut self, value: u16) -> Result<(), DiagnosticJournalError> {
        self.write_slice(&value.to_be_bytes())
    }

    fn write_u32(&mut self, value: u32) -> Result<(), DiagnosticJournalError> {
        self.write_slice(&value.to_be_bytes())
    }

    fn write_u64(&mut self, value: u64) -> Result<(), DiagnosticJournalError> {
        self.write_slice(&value.to_be_bytes())
    }

    fn write_slice(&mut self, value: &[u8]) -> Result<(), DiagnosticJournalError> {
        let end = self
            .cursor
            .checked_add(value.len())
            .ok_or(DiagnosticJournalError::InvalidLength)?;
        let destination = self
            .bytes
            .get_mut(self.cursor..end)
            .ok_or(DiagnosticJournalError::InvalidLength)?;
        destination.copy_from_slice(value);
        self.cursor = end;
        Ok(())
    }

    fn skip(&mut self, length: usize) -> Result<(), DiagnosticJournalError> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or(DiagnosticJournalError::InvalidLength)?;
        if end > N {
            return Err(DiagnosticJournalError::InvalidLength);
        }
        self.cursor = end;
        Ok(())
    }

    fn finish(mut self) -> Result<[u8; N], DiagnosticJournalError> {
        if self.cursor != N {
            return Err(DiagnosticJournalError::InvalidLength);
        }
        Ok(core::mem::replace(&mut self.bytes, [0; N]))
    }
}

impl<const N: usize> Drop for FixedWriter<N> {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

struct FixedReader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> FixedReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn read_u8(&mut self) -> Result<u8, DiagnosticJournalError> {
        Ok(self.read_slice(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, DiagnosticJournalError> {
        Ok(u16::from_be_bytes(self.read_array()?))
    }

    fn read_u32(&mut self) -> Result<u32, DiagnosticJournalError> {
        Ok(u32::from_be_bytes(self.read_array()?))
    }

    fn read_u64(&mut self) -> Result<u64, DiagnosticJournalError> {
        Ok(u64::from_be_bytes(self.read_array()?))
    }

    fn read_array<const M: usize>(&mut self) -> Result<[u8; M], DiagnosticJournalError> {
        self.read_slice(M)?
            .try_into()
            .map_err(|_| DiagnosticJournalError::InvalidLength)
    }

    fn read_slice(&mut self, length: usize) -> Result<&'a [u8], DiagnosticJournalError> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or(DiagnosticJournalError::InvalidLength)?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .ok_or(DiagnosticJournalError::InvalidLength)?;
        self.cursor = end;
        Ok(value)
    }

    const fn is_finished(&self) -> bool {
        self.cursor == self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use bli_kvm_diagnostic_export::{
        DiagnosticComponent, DiagnosticEvent, DiagnosticExporter, DiagnosticRecord, RejectReason,
    };
    use sha2::{Digest, Sha256};

    use super::*;

    fn key(id: u32, value: u8) -> DiagnosticJournalKey {
        DiagnosticJournalKey::new(id, [value; DIAGNOSTIC_JOURNAL_KEY_BYTES]).unwrap()
    }

    fn nonce(value: u8) -> DiagnosticJournalNonce {
        DiagnosticJournalNonce::new([value; DIAGNOSTIC_JOURNAL_NONCE_BYTES]).unwrap()
    }

    fn bundle(offset: u32) -> DiagnosticBundle {
        let mut exporter = DiagnosticExporter::new();
        exporter
            .push(
                DiagnosticRecord::new(
                    offset,
                    DiagnosticComponent::ControlBroker,
                    DiagnosticEvent::RequestRejected {
                        reason: RejectReason::CapabilityDisabled,
                    },
                )
                .unwrap(),
            )
            .unwrap();
        exporter.export().unwrap()
    }

    fn snapshot<'a>(
        instance_id: u64,
        window_id: u32,
        bundle: &'a DiagnosticBundle,
    ) -> DiagnosticSnapshot<'a> {
        DiagnosticSnapshot::new(
            DiagnosticBinding::new(instance_id, window_id).unwrap(),
            1,
            bundle,
        )
        .unwrap()
    }

    fn initialized<'a>(
        key: &'a DiagnosticJournalKey,
    ) -> (RawDiagnosticJournalCopy, DiagnosticJournalKeyRing<'a>) {
        let ring = DiagnosticJournalKeyRing::new(key, None).unwrap();
        let plan = plan_initialize(&DiagnosticJournal::empty(), &ring, nonce(1)).unwrap();
        (*plan.bytes(), ring)
    }

    fn select(
        bytes: &RawDiagnosticJournalCopy,
        ring: &DiagnosticJournalKeyRing<'_>,
    ) -> SelectedDiagnosticJournal {
        select_copies(Some(bytes), Some(bytes), ring, 1).unwrap()
    }

    fn commit(
        selected: &SelectedDiagnosticJournal,
        mutation: &JournalMutation,
        nonce: DiagnosticJournalNonce,
        ring: &DiagnosticJournalKeyRing<'_>,
    ) -> (RawDiagnosticJournalCopy, SelectedDiagnosticJournal) {
        let plan = plan_update(selected, mutation, ring, nonce).unwrap();
        let bytes = *plan.bytes();
        let selected = select_copies(Some(&bytes), Some(&bytes), ring, plan.generation()).unwrap();
        (bytes, selected)
    }

    #[test]
    fn fixed_sizes_and_key_nonce_guards_are_exact() {
        assert_eq!(DIAGNOSTIC_JOURNAL_ENTRY_BYTES, 8_220);
        assert_eq!(DIAGNOSTIC_JOURNAL_PLAIN_BYTES, 16_472);
        assert_eq!(DIAGNOSTIC_JOURNAL_COPY_BYTES, 16_552);
        assert!(matches!(
            DiagnosticJournalKey::new(0, [1; 32]),
            Err(DiagnosticJournalError::InvalidKey)
        ));
        assert!(matches!(
            DiagnosticJournalKey::new(1, [0; 32]),
            Err(DiagnosticJournalError::InvalidKey)
        ));
        assert!(DiagnosticJournalNonce::new([0; 24]).is_none());
        let active = key(1, 1);
        assert!(matches!(
            DiagnosticJournalKeyRing::new(&active, Some(&active)),
            Err(DiagnosticJournalError::DuplicateKeyId)
        ));
    }

    #[test]
    fn empty_copy_has_stable_format_vector() {
        let key = key(7, 3);
        let ring = DiagnosticJournalKeyRing::new(&key, None).unwrap();
        let plan = plan_initialize(&DiagnosticJournal::empty(), &ring, nonce(1)).unwrap();
        let digest: [u8; 32] = Sha256::digest(plan.bytes()).into();
        assert_eq!(
            digest,
            [
                114, 239, 212, 23, 236, 118, 171, 161, 99, 78, 120, 114, 30, 210, 60, 108, 128,
                195, 70, 170, 68, 210, 206, 78, 118, 156, 82, 114, 74, 54, 151, 101,
            ]
        );
    }

    #[test]
    fn encrypted_copy_round_trips_and_hides_plaintext() {
        let key = key(7, 3);
        let (bytes, ring) = initialized(&key);
        assert_eq!(&bytes[..8], &DIAGNOSTIC_JOURNAL_COPY_MAGIC);
        assert!(
            !bytes
                .windows(DIAGNOSTIC_JOURNAL_PLAIN_MAGIC.len())
                .any(|window| window == DIAGNOSTIC_JOURNAL_PLAIN_MAGIC)
        );
        let selected = select(&bytes, &ring);
        assert_eq!(selected.generation(), 1);
        assert_eq!(selected.journal().entry_count(), 0);
        assert!(!selected.degraded());
        assert!(std::format!("{selected:?}").contains("entry_count: 0"));
    }

    #[test]
    fn tamper_wrong_key_and_missing_key_fail_closed() {
        let correct_key = key(7, 3);
        let wrong = key(7, 4);
        let unknown = key(8, 3);
        let (bytes, ring) = initialized(&correct_key);
        let wrong_ring = DiagnosticJournalKeyRing::new(&wrong, None).unwrap();
        assert_eq!(
            open_copy(&bytes, &wrong_ring).err(),
            Some(DiagnosticJournalError::AuthenticationFailed)
        );
        let unknown_ring = DiagnosticJournalKeyRing::new(&unknown, None).unwrap();
        assert_eq!(
            open_copy(&bytes, &unknown_ring).err(),
            Some(DiagnosticJournalError::KeyUnavailable)
        );
        for index in [
            0,
            10,
            24,
            DIAGNOSTIC_JOURNAL_COPY_HEADER_BYTES,
            bytes.len() - 1,
        ] {
            let mut tampered = bytes;
            tampered[index] ^= 1;
            assert!(open_copy(&tampered, &ring).is_err());
        }
    }

    #[test]
    fn new_snapshot_requires_persisted_selection_before_acknowledge() {
        let key = key(1, 5);
        let (initial, ring) = initialized(&key);
        let selected = select(&initial, &ring);
        let bundle = bundle(1);
        let observed = CollectorObservation::Sealed(snapshot(9, 4, &bundle));
        let plan = plan_reconcile(&selected, observed).unwrap();
        assert_eq!(plan.action(), ReconciliationAction::Persist);
        let mutation = plan.mutation().unwrap();
        assert_eq!(mutation.expected_generation(), 1);
        assert_eq!(mutation.generation(), 2);
        assert_eq!(
            mutation
                .journal()
                .entry(DiagnosticBinding::new(9, 4).unwrap())
                .unwrap()
                .state(),
            DiagnosticJournalEntryState::PendingAcknowledgement
        );
        let entry = mutation
            .journal()
            .entry(DiagnosticBinding::new(9, 4).unwrap())
            .unwrap();
        assert!(!std::format!("{entry:?}").contains("schema="));
        assert!(!std::format!("{:?}", mutation.journal()).contains("capability_disabled"));

        let (_bytes, selected) = commit(&selected, mutation, nonce(2), &ring);
        let plan = plan_reconcile(&selected, observed).unwrap();
        assert_eq!(
            plan.action(),
            ReconciliationAction::Acknowledge(DiagnosticBinding::new(9, 4).unwrap())
        );
        assert!(plan.mutation().is_none());
    }

    #[test]
    fn ambiguous_ack_requires_read_and_persist_before_retry() {
        let key = key(1, 5);
        let (initial, ring) = initialized(&key);
        let selected = select(&initial, &ring);
        let bundle = bundle(1);
        let observed = CollectorObservation::Sealed(snapshot(9, 4, &bundle));
        let store = plan_reconcile(&selected, observed).unwrap();
        let (_bytes, pending) = commit(&selected, store.mutation().unwrap(), nonce(2), &ring);
        let ambiguous = plan_acknowledgement_result(
            &pending,
            DiagnosticBinding::new(9, 4).unwrap(),
            AcknowledgementResult::DeliveryAmbiguous,
        )
        .unwrap();
        assert_eq!(ambiguous.action(), ReconciliationAction::Persist);
        let (_bytes, ambiguous_selected) =
            commit(&pending, ambiguous.mutation().unwrap(), nonce(3), &ring);
        let reread = plan_reconcile(&ambiguous_selected, observed).unwrap();
        assert_eq!(reread.action(), ReconciliationAction::Persist);
        let (_bytes, retryable) = commit(
            &ambiguous_selected,
            reread.mutation().unwrap(),
            nonce(4),
            &ring,
        );
        assert_eq!(
            plan_reconcile(&retryable, observed).unwrap().action(),
            ReconciliationAction::Acknowledge(DiagnosticBinding::new(9, 4).unwrap())
        );
    }

    #[test]
    fn no_sealed_marks_pending_or_ambiguous_as_orphaned() {
        let key = key(1, 5);
        let (initial, ring) = initialized(&key);
        let selected = select(&initial, &ring);
        let bundle = bundle(1);
        let store = plan_reconcile(
            &selected,
            CollectorObservation::Sealed(snapshot(9, 4, &bundle)),
        )
        .unwrap();
        let (_bytes, pending) = commit(&selected, store.mutation().unwrap(), nonce(2), &ring);
        let terminal = plan_reconcile(&pending, CollectorObservation::NoSealed).unwrap();
        assert_eq!(terminal.action(), ReconciliationAction::Persist);
        assert_eq!(
            terminal
                .mutation()
                .unwrap()
                .journal()
                .entry(DiagnosticBinding::new(9, 4).unwrap())
                .unwrap()
                .state(),
            DiagnosticJournalEntryState::Orphaned
        );
    }

    #[test]
    fn different_sealed_binding_orphans_before_storing_new() {
        let key = key(1, 5);
        let (initial, ring) = initialized(&key);
        let selected = select(&initial, &ring);
        let first_bundle = bundle(1);
        let first = CollectorObservation::Sealed(snapshot(9, 4, &first_bundle));
        let store = plan_reconcile(&selected, first).unwrap();
        let (_bytes, pending) = commit(&selected, store.mutation().unwrap(), nonce(2), &ring);
        let second_bundle = bundle(2);
        let second = CollectorObservation::Sealed(snapshot(10, 1, &second_bundle));
        let orphan = plan_reconcile(&pending, second).unwrap();
        assert_eq!(orphan.action(), ReconciliationAction::Persist);
        assert_eq!(
            orphan
                .mutation()
                .unwrap()
                .journal()
                .entry(DiagnosticBinding::new(9, 4).unwrap())
                .unwrap()
                .state(),
            DiagnosticJournalEntryState::Orphaned
        );
        let (_bytes, orphaned) = commit(&pending, orphan.mutation().unwrap(), nonce(3), &ring);
        let store_second = plan_reconcile(&orphaned, second).unwrap();
        assert_eq!(store_second.action(), ReconciliationAction::Persist);
        assert_eq!(store_second.mutation().unwrap().journal().entry_count(), 2);
    }

    #[test]
    fn mismatch_reads_again_and_applied_ack_plans_terminal_persist() {
        let key = key(1, 5);
        let (initial, ring) = initialized(&key);
        let selected = select(&initial, &ring);
        let bundle = bundle(1);
        let store = plan_reconcile(
            &selected,
            CollectorObservation::Sealed(snapshot(9, 4, &bundle)),
        )
        .unwrap();
        let (_bytes, pending) = commit(&selected, store.mutation().unwrap(), nonce(2), &ring);
        let binding = DiagnosticBinding::new(9, 4).unwrap();
        assert_eq!(
            plan_acknowledgement_result(&pending, binding, AcknowledgementResult::Mismatch)
                .unwrap()
                .action(),
            ReconciliationAction::ReadCollector
        );
        let applied =
            plan_acknowledgement_result(&pending, binding, AcknowledgementResult::Applied).unwrap();
        assert_eq!(applied.action(), ReconciliationAction::Persist);
        assert_eq!(
            applied
                .mutation()
                .unwrap()
                .journal()
                .entry(binding)
                .unwrap()
                .state(),
            DiagnosticJournalEntryState::Acknowledged
        );
        let absent =
            plan_acknowledgement_result(&pending, binding, AcknowledgementResult::NoSealed)
                .unwrap();
        assert_eq!(
            absent
                .mutation()
                .unwrap()
                .journal()
                .entry(binding)
                .unwrap()
                .state(),
            DiagnosticJournalEntryState::Orphaned
        );
    }

    #[test]
    fn redundant_selection_repair_gap_split_brain_and_floor_fail_closed() {
        let key = key(1, 5);
        let (first, ring) = initialized(&key);
        let degraded = select_copies(Some(&first), None, &ring, 1).unwrap();
        assert!(degraded.degraded());
        assert_eq!(degraded.repair_target(), Some(CopyIndex::B));
        let repair = plan_repair(&degraded).unwrap();
        assert_eq!(repair.target(), CopyIndex::B);
        assert_eq!(repair.bytes(), &first);

        let selected = select(&first, &ring);
        let bundle = bundle(1);
        let mutation = plan_reconcile(
            &selected,
            CollectorObservation::Sealed(snapshot(9, 4, &bundle)),
        )
        .unwrap();
        let update = plan_update(&selected, mutation.mutation().unwrap(), &ring, nonce(2)).unwrap();
        let second = *update.bytes();
        assert_eq!(
            select_copies(Some(&first), Some(&second), &ring, 1)
                .unwrap()
                .generation(),
            2
        );
        assert_eq!(
            select_copies(Some(&first), Some(&second), &ring, 3).err(),
            Some(DiagnosticJournalError::RollbackDetected)
        );

        let second_selected = select(&second, &ring);
        let no_sealed = plan_reconcile(&second_selected, CollectorObservation::NoSealed).unwrap();
        let third = *plan_update(
            &second_selected,
            no_sealed.mutation().unwrap(),
            &ring,
            nonce(3),
        )
        .unwrap()
        .bytes();
        assert_eq!(
            select_copies(Some(&first), Some(&third), &ring, 1).err(),
            Some(DiagnosticJournalError::GenerationGap)
        );

        let mut split = second;
        split[DIAGNOSTIC_JOURNAL_COPY_HEADER_BYTES] ^= 1;
        assert!(select_copies(Some(&second), Some(&split), &ring, 1).is_ok());
        let other_nonce = seal_copy(second_selected.journal(), &key, nonce(9)).unwrap();
        assert_eq!(
            select_copies(Some(&second), Some(&other_nonce), &ring, 1).err(),
            Some(DiagnosticJournalError::SplitBrain)
        );
    }

    #[test]
    fn nonce_reuse_is_rejected_and_previous_key_can_be_read() {
        let old = key(1, 5);
        let active = key(2, 6);
        let old_ring = DiagnosticJournalKeyRing::new(&old, None).unwrap();
        let old_plan = plan_initialize(&DiagnosticJournal::empty(), &old_ring, nonce(1)).unwrap();
        let ring = DiagnosticJournalKeyRing::new(&active, Some(&old)).unwrap();
        let selected =
            select_copies(Some(old_plan.bytes()), Some(old_plan.bytes()), &ring, 1).unwrap();
        let bundle = bundle(1);
        let mutation = plan_reconcile(
            &selected,
            CollectorObservation::Sealed(snapshot(9, 4, &bundle)),
        )
        .unwrap();
        assert!(plan_update(&selected, mutation.mutation().unwrap(), &ring, nonce(1)).is_ok());

        let same_key_ring = DiagnosticJournalKeyRing::new(&old, None).unwrap();
        let old_selected = select(old_plan.bytes(), &same_key_ring);
        assert_eq!(
            plan_update(
                &old_selected,
                &mutation_for_empty_append(&old_selected, &bundle),
                &same_key_ring,
                nonce(1),
            )
            .err(),
            Some(DiagnosticJournalError::NonceReuse)
        );
    }

    fn mutation_for_empty_append<'a>(
        selected: &'a SelectedDiagnosticJournal,
        bundle: &'a DiagnosticBundle,
    ) -> JournalMutation {
        plan_reconcile(
            selected,
            CollectorObservation::Sealed(snapshot(9, 4, bundle)),
        )
        .unwrap()
        .mutation
        .unwrap()
    }

    #[test]
    fn retention_evicts_only_oldest_terminal_entry() {
        let mut journal = DiagnosticJournal::empty();
        journal.generation = 1;
        for index in 0..MAX_DIAGNOSTIC_JOURNAL_ENTRIES {
            let bundle = bundle(index as u32);
            let mutation = journal
                .plan_append(snapshot(10 + index as u64, 1, &bundle))
                .unwrap();
            journal = mutation.journal;
            let active = journal.active_index().unwrap();
            journal = journal
                .plan_state(active, DiagnosticJournalEntryState::Acknowledged)
                .unwrap()
                .journal;
        }
        assert_eq!(journal.entry_count(), MAX_DIAGNOSTIC_JOURNAL_ENTRIES);
        let first_id = journal.entries().next().unwrap().entry_id();
        let new_bundle = bundle(99);
        let mutation = journal.plan_append(snapshot(99, 1, &new_bundle)).unwrap();
        assert_eq!(
            mutation.journal.entry_count(),
            MAX_DIAGNOSTIC_JOURNAL_ENTRIES
        );
        assert!(mutation.journal.entries().next().unwrap().entry_id() > first_id);
    }
}
