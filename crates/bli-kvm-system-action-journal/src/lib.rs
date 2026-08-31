#![no_std]
#![forbid(unsafe_code)]

//! Canonical redundant-copy journal model for system-action policy state.
//!
//! This crate encodes validated policy snapshots, detects corruption with a
//! domain-separated SHA-256 digest, selects between two generations, and plans
//! write ordering that preserves one previously valid copy. It has no storage,
//! filesystem, process, network, clock, or actuator access.

use bli_kvm_system_action_policy::{
    MAX_OPERATION_RECORDS, STATE_SCHEMA_VERSION, SystemAction, SystemActionPhase,
    SystemActionPolicyEngine, SystemActionPolicyState, SystemActionRecordSnapshot,
};
use sha2::{Digest, Sha256};

pub const JOURNAL_COPY_SCHEMA_VERSION: u16 = 1;
pub const JOURNAL_COPY_MAGIC: [u8; 8] = *b"BLKSAJ01";
pub const JOURNAL_DOMAIN: &[u8] = b"hardened-blikvm-system-action-journal-v1\0";

pub const POLICY_RECORD_BYTES: usize = 126;
pub const POLICY_STATE_HEADER_BYTES: usize = 10;
pub const POLICY_STATE_BYTES: usize =
    POLICY_STATE_HEADER_BYTES + MAX_OPERATION_RECORDS * POLICY_RECORD_BYTES;
pub const JOURNAL_COPY_HEADER_BYTES: usize = 18;
pub const JOURNAL_CHECKSUM_BYTES: usize = 32;
pub const JOURNAL_CHECKSUM_OFFSET: usize = JOURNAL_COPY_HEADER_BYTES + POLICY_STATE_BYTES;
pub const JOURNAL_COPY_BYTES: usize = JOURNAL_CHECKSUM_OFFSET + JOURNAL_CHECKSUM_BYTES;

pub type EncodedPolicyState = [u8; POLICY_STATE_BYTES];
pub type RawJournalCopy = [u8; JOURNAL_COPY_BYTES];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalError {
    InvalidLength,
    InvalidMagic,
    UnsupportedJournalSchema,
    UnsupportedPolicySchema,
    InvalidChecksum,
    InvalidEncoding,
    InvalidPolicyState,
    InvalidGeneration,
    GenerationMismatch,
    NoValidCopy,
    SplitBrain,
    GenerationGap,
    RollbackDetected,
    ConcurrentStateChange,
    StateGenerationGap,
    VerificationFailed,
    GenerationExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalCopy {
    generation: u64,
    state: SystemActionPolicyState,
}

impl JournalCopy {
    pub fn new(state: SystemActionPolicyState) -> Result<Self, JournalError> {
        SystemActionPolicyEngine::restore(state).map_err(|_| JournalError::InvalidPolicyState)?;
        let generation = state
            .generation()
            .checked_add(1)
            .ok_or(JournalError::GenerationExhausted)?;
        Ok(Self { generation, state })
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, JournalError> {
        if bytes.len() != JOURNAL_COPY_BYTES {
            return Err(JournalError::InvalidLength);
        }
        if bytes[..JOURNAL_COPY_MAGIC.len()] != JOURNAL_COPY_MAGIC {
            return Err(JournalError::InvalidMagic);
        }
        let schema_version = u16::from_be_bytes(
            bytes[8..10]
                .try_into()
                .map_err(|_| JournalError::InvalidEncoding)?,
        );
        if schema_version != JOURNAL_COPY_SCHEMA_VERSION {
            return Err(JournalError::UnsupportedJournalSchema);
        }
        let generation = u64::from_be_bytes(
            bytes[10..18]
                .try_into()
                .map_err(|_| JournalError::InvalidEncoding)?,
        );
        if generation == 0 {
            return Err(JournalError::InvalidGeneration);
        }

        let expected_checksum = checksum_prefix(&bytes[..JOURNAL_CHECKSUM_OFFSET]);
        if !constant_time_equal(
            &expected_checksum,
            &bytes[JOURNAL_CHECKSUM_OFFSET..JOURNAL_COPY_BYTES],
        ) {
            return Err(JournalError::InvalidChecksum);
        }

        let state =
            decode_policy_state(&bytes[JOURNAL_COPY_HEADER_BYTES..JOURNAL_CHECKSUM_OFFSET])?;
        if state
            .generation()
            .checked_add(1)
            .ok_or(JournalError::GenerationExhausted)?
            != generation
        {
            return Err(JournalError::GenerationMismatch);
        }
        let copy = Self { generation, state };
        if copy.to_bytes()?.as_slice() != bytes {
            return Err(JournalError::InvalidEncoding);
        }
        Ok(copy)
    }

    pub fn to_bytes(self) -> Result<RawJournalCopy, JournalError> {
        let payload = encode_policy_state(&self.state)?;
        let mut bytes = [0u8; JOURNAL_COPY_BYTES];
        bytes[..8].copy_from_slice(&JOURNAL_COPY_MAGIC);
        bytes[8..10].copy_from_slice(&JOURNAL_COPY_SCHEMA_VERSION.to_be_bytes());
        bytes[10..18].copy_from_slice(&self.generation.to_be_bytes());
        bytes[JOURNAL_COPY_HEADER_BYTES..JOURNAL_CHECKSUM_OFFSET].copy_from_slice(&payload);
        let checksum = checksum_prefix(&bytes[..JOURNAL_CHECKSUM_OFFSET]);
        bytes[JOURNAL_CHECKSUM_OFFSET..].copy_from_slice(&checksum);
        Ok(bytes)
    }

    pub const fn generation(self) -> u64 {
        self.generation
    }

    pub const fn state(self) -> SystemActionPolicyState {
        self.state
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedJournal {
    selected_index: CopyIndex,
    copy: JournalCopy,
    degraded: bool,
    repair_target: Option<CopyIndex>,
}

impl SelectedJournal {
    pub const fn selected_index(self) -> CopyIndex {
        self.selected_index
    }

    pub const fn generation(self) -> u64 {
        self.copy.generation
    }

    pub const fn state(self) -> SystemActionPolicyState {
        self.copy.state
    }

    pub const fn degraded(self) -> bool {
        self.degraded
    }

    pub const fn repair_target(self) -> Option<CopyIndex> {
        self.repair_target
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalWritePlan {
    expected_generation: u64,
    generation: u64,
    first: CopyIndex,
    second: CopyIndex,
    bytes: RawJournalCopy,
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

    pub const fn bytes(&self) -> &RawJournalCopy {
        &self.bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRepairPlan {
    expected_generation: u64,
    generation: u64,
    target: CopyIndex,
    bytes: RawJournalCopy,
}

impl JournalRepairPlan {
    pub const fn expected_generation(&self) -> u64 {
        self.expected_generation
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn target(&self) -> CopyIndex {
        self.target
    }

    pub const fn bytes(&self) -> &RawJournalCopy {
        &self.bytes
    }
}

pub fn encode_policy_state(
    state: &SystemActionPolicyState,
) -> Result<EncodedPolicyState, JournalError> {
    SystemActionPolicyEngine::restore(*state).map_err(|_| JournalError::InvalidPolicyState)?;
    let mut writer = FixedWriter::<POLICY_STATE_BYTES>::new();
    writer.write_u16(STATE_SCHEMA_VERSION)?;
    writer.write_u64(state.generation())?;
    for snapshot in state.record_snapshots() {
        match snapshot {
            Some(snapshot) => encode_snapshot(&mut writer, snapshot)?,
            None => {
                writer.write_u8(0)?;
                writer.skip(POLICY_RECORD_BYTES - 1)?;
            }
        }
    }
    writer.finish()
}

pub fn decode_policy_state(bytes: &[u8]) -> Result<SystemActionPolicyState, JournalError> {
    if bytes.len() != POLICY_STATE_BYTES {
        return Err(JournalError::InvalidLength);
    }
    let mut reader = FixedReader::new(bytes);
    if reader.read_u16()? != STATE_SCHEMA_VERSION {
        return Err(JournalError::UnsupportedPolicySchema);
    }
    let generation = reader.read_u64()?;
    let mut snapshots = [None; MAX_OPERATION_RECORDS];
    for slot in &mut snapshots {
        match reader.read_u8()? {
            0 => {
                if reader
                    .read_slice(POLICY_RECORD_BYTES - 1)?
                    .iter()
                    .any(|byte| *byte != 0)
                {
                    return Err(JournalError::InvalidEncoding);
                }
            }
            1 => *slot = Some(decode_snapshot(&mut reader)?),
            _ => return Err(JournalError::InvalidEncoding),
        }
    }
    if !reader.is_finished() {
        return Err(JournalError::InvalidLength);
    }
    let state = SystemActionPolicyState::from_record_snapshots(generation, snapshots)
        .map_err(|_| JournalError::InvalidPolicyState)?;
    if encode_policy_state(&state)?.as_slice() != bytes {
        return Err(JournalError::InvalidEncoding);
    }
    Ok(state)
}

pub fn select_copies(
    copy_a: Option<&[u8]>,
    copy_b: Option<&[u8]>,
    minimum_generation: u64,
) -> Result<SelectedJournal, JournalError> {
    let a = copy_a.and_then(|bytes| JournalCopy::from_bytes(bytes).ok());
    let b = copy_b.and_then(|bytes| JournalCopy::from_bytes(bytes).ok());
    let selected = match (a, b) {
        (None, None) => return Err(JournalError::NoValidCopy),
        (Some(copy), None) => SelectedJournal {
            selected_index: CopyIndex::A,
            copy,
            degraded: true,
            repair_target: Some(CopyIndex::B),
        },
        (None, Some(copy)) => SelectedJournal {
            selected_index: CopyIndex::B,
            copy,
            degraded: true,
            repair_target: Some(CopyIndex::A),
        },
        (Some(a), Some(b)) if a.generation == b.generation => {
            if a.state != b.state {
                return Err(JournalError::SplitBrain);
            }
            SelectedJournal {
                selected_index: CopyIndex::A,
                copy: a,
                degraded: false,
                repair_target: None,
            }
        }
        (Some(a), Some(b)) => {
            if a.generation.abs_diff(b.generation) != 1 {
                return Err(JournalError::GenerationGap);
            }
            if a.generation > b.generation {
                SelectedJournal {
                    selected_index: CopyIndex::A,
                    copy: a,
                    degraded: true,
                    repair_target: Some(CopyIndex::B),
                }
            } else {
                SelectedJournal {
                    selected_index: CopyIndex::B,
                    copy: b,
                    degraded: true,
                    repair_target: Some(CopyIndex::A),
                }
            }
        }
    };
    if selected.generation() < minimum_generation {
        return Err(JournalError::RollbackDetected);
    }
    Ok(selected)
}

pub fn plan_initialize(state: SystemActionPolicyState) -> Result<JournalWritePlan, JournalError> {
    if state.generation() != 0 || state.records().iter().any(Option::is_some) {
        return Err(JournalError::StateGenerationGap);
    }
    let copy = JournalCopy::new(state)?;
    Ok(JournalWritePlan {
        expected_generation: 0,
        generation: copy.generation,
        first: CopyIndex::A,
        second: CopyIndex::B,
        bytes: copy.to_bytes()?,
    })
}

pub fn plan_update(
    selected: &SelectedJournal,
    expected_generation: u64,
    next_state: SystemActionPolicyState,
) -> Result<JournalWritePlan, JournalError> {
    if selected.generation() != expected_generation {
        return Err(JournalError::ConcurrentStateChange);
    }
    let expected_state_generation = selected
        .state()
        .generation()
        .checked_add(1)
        .ok_or(JournalError::GenerationExhausted)?;
    if next_state.generation() != expected_state_generation {
        return Err(JournalError::StateGenerationGap);
    }
    let copy = JournalCopy::new(next_state)?;
    if copy.generation
        != selected
            .generation()
            .checked_add(1)
            .ok_or(JournalError::GenerationExhausted)?
    {
        return Err(JournalError::GenerationMismatch);
    }
    Ok(JournalWritePlan {
        expected_generation,
        generation: copy.generation,
        first: selected.selected_index.other(),
        second: selected.selected_index,
        bytes: copy.to_bytes()?,
    })
}

pub fn plan_repair(selected: &SelectedJournal) -> Result<Option<JournalRepairPlan>, JournalError> {
    let Some(target) = selected.repair_target else {
        return Ok(None);
    };
    let copy = JournalCopy::new(selected.state())?;
    if copy.generation != selected.generation() {
        return Err(JournalError::GenerationMismatch);
    }
    Ok(Some(JournalRepairPlan {
        expected_generation: selected.generation(),
        generation: copy.generation,
        target,
        bytes: copy.to_bytes()?,
    }))
}

pub fn verify_written(expected: &RawJournalCopy, observed: &[u8]) -> Result<(), JournalError> {
    let decoded = JournalCopy::from_bytes(observed)?;
    if decoded.to_bytes()?.as_slice() != expected {
        return Err(JournalError::VerificationFailed);
    }
    Ok(())
}

fn encode_snapshot(
    writer: &mut FixedWriter<POLICY_STATE_BYTES>,
    snapshot: SystemActionRecordSnapshot,
) -> Result<(), JournalError> {
    writer.write_u8(1)?;
    writer.write_array(&snapshot.operation_id)?;
    writer.write_u64(snapshot.session_binding)?;
    writer.write_array(&snapshot.confirmation_binding)?;
    writer.write_u64(snapshot.confirmation_issued_at_ms)?;
    writer.write_array(&snapshot.origin_boot_binding)?;
    writer.write_u8(match snapshot.action {
        SystemAction::Reboot => 0,
        SystemAction::Shutdown => 1,
    })?;
    writer.write_u8(match snapshot.phase {
        SystemActionPhase::Prepared => 0,
        SystemActionPhase::Dispatching => 1,
        SystemActionPhase::PostBootAccepted => 2,
        SystemActionPhase::Rejected => 3,
    })?;
    writer.write_u64(snapshot.prepared_at_ms)?;
    writer.write_option_u64(snapshot.dispatch_started_at_ms)?;
    writer.write_option_u64(snapshot.terminal_at_ms)?;
    writer.write_option_array(snapshot.terminal_boot_binding)?;
    Ok(())
}

fn decode_snapshot(
    reader: &mut FixedReader<'_>,
) -> Result<SystemActionRecordSnapshot, JournalError> {
    let operation_id = reader.read_array()?;
    let session_binding = reader.read_u64()?;
    let confirmation_binding = reader.read_array()?;
    let confirmation_issued_at_ms = reader.read_u64()?;
    let origin_boot_binding = reader.read_array()?;
    let action = match reader.read_u8()? {
        0 => SystemAction::Reboot,
        1 => SystemAction::Shutdown,
        _ => return Err(JournalError::InvalidEncoding),
    };
    let phase = match reader.read_u8()? {
        0 => SystemActionPhase::Prepared,
        1 => SystemActionPhase::Dispatching,
        2 => SystemActionPhase::PostBootAccepted,
        3 => SystemActionPhase::Rejected,
        _ => return Err(JournalError::InvalidEncoding),
    };
    Ok(SystemActionRecordSnapshot {
        operation_id,
        session_binding,
        confirmation_binding,
        confirmation_issued_at_ms,
        origin_boot_binding,
        action,
        phase,
        prepared_at_ms: reader.read_u64()?,
        dispatch_started_at_ms: reader.read_option_u64()?,
        terminal_at_ms: reader.read_option_u64()?,
        terminal_boot_binding: reader.read_option_array()?,
    })
}

fn checksum_prefix(prefix: &[u8]) -> [u8; JOURNAL_CHECKSUM_BYTES] {
    let mut hasher = Sha256::new();
    hasher.update(JOURNAL_DOMAIN);
    hasher.update(prefix);
    let digest = hasher.finalize();
    let mut checksum = [0u8; JOURNAL_CHECKSUM_BYTES];
    checksum.copy_from_slice(&digest);
    checksum
}

fn constant_time_equal(expected: &[u8; JOURNAL_CHECKSUM_BYTES], observed: &[u8]) -> bool {
    if observed.len() != JOURNAL_CHECKSUM_BYTES {
        return false;
    }
    let mut difference = 0u8;
    for (expected, observed) in expected.iter().zip(observed) {
        difference |= expected ^ observed;
    }
    difference == 0
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

    fn write_u8(&mut self, value: u8) -> Result<(), JournalError> {
        self.write_slice(&[value])
    }

    fn write_u16(&mut self, value: u16) -> Result<(), JournalError> {
        self.write_slice(&value.to_be_bytes())
    }

    fn write_u64(&mut self, value: u64) -> Result<(), JournalError> {
        self.write_slice(&value.to_be_bytes())
    }

    fn write_array<const M: usize>(&mut self, value: &[u8; M]) -> Result<(), JournalError> {
        self.write_slice(value)
    }

    fn write_option_u64(&mut self, value: Option<u64>) -> Result<(), JournalError> {
        match value {
            Some(value) => {
                self.write_u8(1)?;
                self.write_u64(value)
            }
            None => {
                self.write_u8(0)?;
                self.write_u64(0)
            }
        }
    }

    fn write_option_array<const M: usize>(
        &mut self,
        value: Option<[u8; M]>,
    ) -> Result<(), JournalError> {
        match value {
            Some(value) => {
                self.write_u8(1)?;
                self.write_array(&value)
            }
            None => {
                self.write_u8(0)?;
                self.skip(M)
            }
        }
    }

    fn write_slice(&mut self, value: &[u8]) -> Result<(), JournalError> {
        let end = self
            .cursor
            .checked_add(value.len())
            .ok_or(JournalError::InvalidLength)?;
        let destination = self
            .bytes
            .get_mut(self.cursor..end)
            .ok_or(JournalError::InvalidLength)?;
        destination.copy_from_slice(value);
        self.cursor = end;
        Ok(())
    }

    fn skip(&mut self, length: usize) -> Result<(), JournalError> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or(JournalError::InvalidLength)?;
        if end > N {
            return Err(JournalError::InvalidLength);
        }
        self.cursor = end;
        Ok(())
    }

    fn finish(self) -> Result<[u8; N], JournalError> {
        if self.cursor != N {
            return Err(JournalError::InvalidLength);
        }
        Ok(self.bytes)
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

    fn read_u8(&mut self) -> Result<u8, JournalError> {
        Ok(self.read_slice(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, JournalError> {
        Ok(u16::from_be_bytes(self.read_array()?))
    }

    fn read_u64(&mut self) -> Result<u64, JournalError> {
        Ok(u64::from_be_bytes(self.read_array()?))
    }

    fn read_array<const M: usize>(&mut self) -> Result<[u8; M], JournalError> {
        self.read_slice(M)?
            .try_into()
            .map_err(|_| JournalError::InvalidLength)
    }

    fn read_option_u64(&mut self) -> Result<Option<u64>, JournalError> {
        let present = self.read_u8()?;
        let value = self.read_u64()?;
        match (present, value) {
            (0, 0) => Ok(None),
            (1, value) => Ok(Some(value)),
            _ => Err(JournalError::InvalidEncoding),
        }
    }

    fn read_option_array<const M: usize>(&mut self) -> Result<Option<[u8; M]>, JournalError> {
        let present = self.read_u8()?;
        let value = self.read_array()?;
        match present {
            0 if value.iter().all(|byte| *byte == 0) => Ok(None),
            1 => Ok(Some(value)),
            _ => Err(JournalError::InvalidEncoding),
        }
    }

    fn read_slice(&mut self, length: usize) -> Result<&'a [u8], JournalError> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or(JournalError::InvalidLength)?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .ok_or(JournalError::InvalidLength)?;
        self.cursor = end;
        Ok(value)
    }

    const fn is_finished(&self) -> bool {
        self.cursor == self.bytes.len()
    }
}
