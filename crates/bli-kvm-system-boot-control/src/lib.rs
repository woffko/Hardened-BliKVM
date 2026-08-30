use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const RECORD_SCHEMA_VERSION: u32 = 1;
pub const MAX_RECORD_BYTES: usize = 64 * 1024;

const JOURNAL_DOMAIN: &[u8] = b"hardened-blikvm-system-journal-v1\0";
const SELECTOR_DOMAIN: &[u8] = b"hardened-blikvm-boot-selector-v1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemSlot {
    A,
    B,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SlotIdentity {
    pub slot: SystemSlot,
    pub sequence: u64,
    pub manifest_sha256: String,
    pub layout_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalPhase {
    WritePrepared,
    CandidateReady,
    BootPrepared,
    BootArmed,
    AwaitingHealth,
    ConfirmationPrepared,
    RollbackPrepared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemRollbackReason {
    WriteInterrupted,
    ArmInterrupted,
    AttemptsExhausted,
    HealthTimeout,
    ExplicitFailure,
    WrongBoot,
    InvalidProof,
    SelectorMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingSystemUpdate {
    pub operation_id: String,
    pub previous: SlotIdentity,
    pub candidate: SlotIdentity,
    pub phase: JournalPhase,
    pub initial_attempts: u8,
    pub remaining_attempts: u8,
    pub selector_generation: Option<u64>,
    pub boot_id: Option<String>,
    pub health_binding: Option<String>,
    pub deadline_monotonic_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemJournal {
    pub layout_id: String,
    pub floor_sequence: u64,
    pub confirmed: SlotIdentity,
    pub pending: Option<PendingSystemUpdate>,
    pub last_rollback: Option<SystemRollbackReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectorPendingState {
    ArmedCandidate,
    CandidateBooted,
    FallbackPrevious,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingSelector {
    pub operation_id: String,
    pub previous_slot: SystemSlot,
    pub candidate_slot: SystemSlot,
    pub candidate_manifest_sha256: String,
    pub initial_attempts: u8,
    pub remaining_attempts: u8,
    pub state: SelectorPendingState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BootSelector {
    pub layout_id: String,
    pub confirmed_slot: SystemSlot,
    pub confirmed_manifest_sha256: String,
    pub pending: Option<PendingSelector>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationCompatibilityProof {
    pub application_sequence: u64,
    pub minimum_system_sequence: u64,
    pub manifest_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JournalCopy {
    schema_version: u32,
    generation: u64,
    payload: SystemJournal,
    checksum: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectorCopy {
    schema_version: u32,
    generation: u64,
    payload: BootSelector,
    checksum: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedRecord<T> {
    pub generation: u64,
    pub payload: T,
    pub degraded: bool,
}

pub type SelectedJournal = SelectedRecord<SystemJournal>;
pub type SelectedSelector = SelectedRecord<BootSelector>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconciliation {
    Clean,
    CandidateUnselected { phase: JournalPhase },
    InterruptedArm,
    FinishJournalBootArm,
    ContinueArmed,
    ObserveCandidateBoot,
    ContinueAwaitingHealth,
    RecordRollback,
    SelectCandidateConfirmation,
    FinishJournalConfirmation,
    SelectPreviousRollback,
    FinishJournalRollback,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum BootControlError {
    #[error("boot-control record is invalid or non-canonical")]
    InvalidDocument,
    #[error("boot-control record checksum is invalid")]
    InvalidChecksum,
    #[error("boot-control record generation is invalid")]
    InvalidGeneration,
    #[error("both redundant boot-control copies are unavailable or invalid")]
    NoValidCopy,
    #[error("redundant boot-control copies diverge at the same generation")]
    SplitBrain,
    #[error("redundant boot-control copy generations have an unsafe gap")]
    GenerationGap,
    #[error("system journal state is invalid")]
    InvalidJournal,
    #[error("boot selector state is invalid")]
    InvalidSelector,
    #[error("system journal and boot selector disagree")]
    StateMismatch,
    #[error("boot selector attempt count increased")]
    AttemptCounterIncreased,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CompatibilityError {
    #[error("application compatibility proof is invalid")]
    InvalidApplicationProof,
    #[error("system journal is invalid for compatibility evaluation")]
    InvalidSystemState,
    #[error("application is incompatible with the confirmed system")]
    IncompatibleWithConfirmedSystem,
    #[error("application would prevent rollback to the confirmed system")]
    IncompatibleWithRollbackAnchor,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordBody<'a, T> {
    schema_version: u32,
    generation: u64,
    payload: &'a T,
}

impl JournalCopy {
    pub fn new(generation: u64, payload: SystemJournal) -> Result<Self, BootControlError> {
        validate_generation(generation)?;
        validate_journal(&payload)?;
        let checksum = record_checksum(JOURNAL_DOMAIN, generation, &payload)?;
        Ok(Self {
            schema_version: RECORD_SCHEMA_VERSION,
            generation,
            payload,
            checksum,
        })
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BootControlError> {
        let record: Self = parse_canonical(bytes)?;
        if record.schema_version != RECORD_SCHEMA_VERSION {
            return Err(BootControlError::InvalidDocument);
        }
        validate_generation(record.generation)?;
        validate_journal(&record.payload)?;
        validate_record_checksum(
            JOURNAL_DOMAIN,
            record.generation,
            &record.payload,
            &record.checksum,
        )?;
        Ok(record)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, BootControlError> {
        serde_json::to_vec(self).map_err(|_| BootControlError::InvalidDocument)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn payload(&self) -> &SystemJournal {
        &self.payload
    }
}

impl SelectorCopy {
    pub fn new(generation: u64, payload: BootSelector) -> Result<Self, BootControlError> {
        validate_generation(generation)?;
        validate_selector(&payload)?;
        let checksum = record_checksum(SELECTOR_DOMAIN, generation, &payload)?;
        Ok(Self {
            schema_version: RECORD_SCHEMA_VERSION,
            generation,
            payload,
            checksum,
        })
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BootControlError> {
        let record: Self = parse_canonical(bytes)?;
        if record.schema_version != RECORD_SCHEMA_VERSION {
            return Err(BootControlError::InvalidDocument);
        }
        validate_generation(record.generation)?;
        validate_selector(&record.payload)?;
        validate_record_checksum(
            SELECTOR_DOMAIN,
            record.generation,
            &record.payload,
            &record.checksum,
        )?;
        Ok(record)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, BootControlError> {
        serde_json::to_vec(self).map_err(|_| BootControlError::InvalidDocument)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn payload(&self) -> &BootSelector {
        &self.payload
    }
}

pub fn select_journal_copies(
    copy_a: Option<&[u8]>,
    copy_b: Option<&[u8]>,
) -> Result<SelectedJournal, BootControlError> {
    let a = copy_a.and_then(|bytes| JournalCopy::from_bytes(bytes).ok());
    let b = copy_b.and_then(|bytes| JournalCopy::from_bytes(bytes).ok());
    select_decoded(
        a.map(|copy| (copy.generation, copy.payload)),
        b.map(|copy| (copy.generation, copy.payload)),
    )
}

pub fn select_selector_copies(
    copy_a: Option<&[u8]>,
    copy_b: Option<&[u8]>,
) -> Result<SelectedSelector, BootControlError> {
    let a = copy_a.and_then(|bytes| SelectorCopy::from_bytes(bytes).ok());
    let b = copy_b.and_then(|bytes| SelectorCopy::from_bytes(bytes).ok());
    select_decoded(
        a.map(|copy| (copy.generation, copy.payload)),
        b.map(|copy| (copy.generation, copy.payload)),
    )
}

pub fn reconcile(
    journal: &SelectedJournal,
    selector: &SelectedSelector,
) -> Result<Reconciliation, BootControlError> {
    validate_journal(&journal.payload)?;
    validate_selector(&selector.payload)?;
    if journal.payload.layout_id != selector.payload.layout_id {
        return Err(BootControlError::StateMismatch);
    }
    let Some(pending) = journal.payload.pending.as_ref() else {
        if selector.payload.pending.is_none()
            && selector_matches_slot(&selector.payload, &journal.payload.confirmed)
        {
            return Ok(Reconciliation::Clean);
        }
        return Err(BootControlError::StateMismatch);
    };

    if let Some(recorded_generation) = pending.selector_generation {
        if selector.generation < recorded_generation {
            return Err(BootControlError::StateMismatch);
        }
    }

    match pending.phase {
        JournalPhase::WritePrepared | JournalPhase::CandidateReady => {
            require_confirmed_previous(&selector.payload, pending)?;
            Ok(Reconciliation::CandidateUnselected {
                phase: pending.phase,
            })
        }
        JournalPhase::BootPrepared => match selector.payload.pending.as_ref() {
            None => {
                require_confirmed_previous(&selector.payload, pending)?;
                Ok(Reconciliation::InterruptedArm)
            }
            Some(selector_pending) => {
                require_matching_pending(pending, selector_pending)?;
                if selector_pending.state != SelectorPendingState::ArmedCandidate
                    || selector_pending.remaining_attempts != pending.initial_attempts
                {
                    return Err(BootControlError::StateMismatch);
                }
                Ok(Reconciliation::FinishJournalBootArm)
            }
        },
        JournalPhase::BootArmed => {
            let selector_pending = require_selector_pending(&selector.payload, pending)?;
            match selector_pending.state {
                SelectorPendingState::ArmedCandidate => {
                    if selector_pending.remaining_attempts != pending.remaining_attempts {
                        return Err(BootControlError::StateMismatch);
                    }
                    Ok(Reconciliation::ContinueArmed)
                }
                SelectorPendingState::CandidateBooted => {
                    if selector_pending.remaining_attempts > pending.remaining_attempts {
                        return Err(BootControlError::AttemptCounterIncreased);
                    }
                    Ok(Reconciliation::ObserveCandidateBoot)
                }
                SelectorPendingState::FallbackPrevious => Ok(Reconciliation::RecordRollback),
            }
        }
        JournalPhase::AwaitingHealth => {
            let selector_pending = require_selector_pending(&selector.payload, pending)?;
            match selector_pending.state {
                SelectorPendingState::CandidateBooted => {
                    if selector_pending.remaining_attempts > pending.remaining_attempts {
                        return Err(BootControlError::AttemptCounterIncreased);
                    }
                    if selector_pending.remaining_attempts != pending.remaining_attempts {
                        return Err(BootControlError::StateMismatch);
                    }
                    Ok(Reconciliation::ContinueAwaitingHealth)
                }
                SelectorPendingState::FallbackPrevious => Ok(Reconciliation::RecordRollback),
                SelectorPendingState::ArmedCandidate => Err(BootControlError::StateMismatch),
            }
        }
        JournalPhase::ConfirmationPrepared => match selector.payload.pending.as_ref() {
            Some(selector_pending) => {
                require_matching_pending(pending, selector_pending)?;
                if selector_pending.state != SelectorPendingState::CandidateBooted
                    || selector_pending.remaining_attempts != pending.remaining_attempts
                {
                    return Err(BootControlError::StateMismatch);
                }
                Ok(Reconciliation::SelectCandidateConfirmation)
            }
            None if selector_matches_slot(&selector.payload, &pending.candidate) => {
                Ok(Reconciliation::FinishJournalConfirmation)
            }
            None => Err(BootControlError::StateMismatch),
        },
        JournalPhase::RollbackPrepared => match selector.payload.pending.as_ref() {
            Some(selector_pending) => {
                require_matching_pending(pending, selector_pending)?;
                if selector_pending.state != SelectorPendingState::FallbackPrevious {
                    return Ok(Reconciliation::SelectPreviousRollback);
                }
                Ok(Reconciliation::FinishJournalRollback)
            }
            None if selector_matches_slot(&selector.payload, &pending.previous) => {
                Ok(Reconciliation::FinishJournalRollback)
            }
            None => Err(BootControlError::StateMismatch),
        },
    }
}

pub fn validate_confirmed_application_for_system_update(
    journal: &SystemJournal,
    application: &ApplicationCompatibilityProof,
) -> Result<(), CompatibilityError> {
    validate_compatibility_inputs(journal, application)?;
    if application.minimum_system_sequence > journal.confirmed.sequence {
        return Err(CompatibilityError::IncompatibleWithConfirmedSystem);
    }
    Ok(())
}

pub fn validate_application_confirmation(
    journal: &SystemJournal,
    application: &ApplicationCompatibilityProof,
) -> Result<(), CompatibilityError> {
    validate_compatibility_inputs(journal, application)?;
    let accepted_sequence = match journal.pending.as_ref() {
        Some(pending) => pending.previous.sequence,
        None => journal.confirmed.sequence,
    };
    if application.minimum_system_sequence > accepted_sequence {
        return Err(if journal.pending.is_some() {
            CompatibilityError::IncompatibleWithRollbackAnchor
        } else {
            CompatibilityError::IncompatibleWithConfirmedSystem
        });
    }
    Ok(())
}

fn validate_compatibility_inputs(
    journal: &SystemJournal,
    application: &ApplicationCompatibilityProof,
) -> Result<(), CompatibilityError> {
    validate_journal(journal).map_err(|_| CompatibilityError::InvalidSystemState)?;
    if application.application_sequence == 0
        || application.minimum_system_sequence == 0
        || !valid_hex(&application.manifest_sha256, 64)
    {
        return Err(CompatibilityError::InvalidApplicationProof);
    }
    Ok(())
}

fn parse_canonical<T>(bytes: &[u8]) -> Result<T, BootControlError>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    if bytes.is_empty() || bytes.len() > MAX_RECORD_BYTES {
        return Err(BootControlError::InvalidDocument);
    }
    let value = serde_json::from_slice(bytes).map_err(|_| BootControlError::InvalidDocument)?;
    let canonical = serde_json::to_vec(&value).map_err(|_| BootControlError::InvalidDocument)?;
    if canonical != bytes {
        return Err(BootControlError::InvalidDocument);
    }
    Ok(value)
}

fn record_checksum<T: Serialize>(
    domain: &[u8],
    generation: u64,
    payload: &T,
) -> Result<String, BootControlError> {
    let body = RecordBody {
        schema_version: RECORD_SCHEMA_VERSION,
        generation,
        payload,
    };
    let bytes = serde_json::to_vec(&body).map_err(|_| BootControlError::InvalidDocument)?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(bytes);
    Ok(hex_lower(hasher.finalize().as_ref()))
}

fn validate_record_checksum<T: Serialize>(
    domain: &[u8],
    generation: u64,
    payload: &T,
    checksum: &str,
) -> Result<(), BootControlError> {
    if !valid_hex(checksum, 64) || record_checksum(domain, generation, payload)? != checksum {
        return Err(BootControlError::InvalidChecksum);
    }
    Ok(())
}

fn select_decoded<T: Clone + PartialEq>(
    a: Option<(u64, T)>,
    b: Option<(u64, T)>,
) -> Result<SelectedRecord<T>, BootControlError> {
    match (a, b) {
        (None, None) => Err(BootControlError::NoValidCopy),
        (Some((generation, payload)), None) | (None, Some((generation, payload))) => {
            Ok(SelectedRecord {
                generation,
                payload,
                degraded: true,
            })
        }
        (Some((generation_a, payload_a)), Some((generation_b, payload_b))) => {
            if generation_a == generation_b {
                if payload_a != payload_b {
                    return Err(BootControlError::SplitBrain);
                }
                return Ok(SelectedRecord {
                    generation: generation_a,
                    payload: payload_a,
                    degraded: false,
                });
            }
            let gap = generation_a.abs_diff(generation_b);
            if gap != 1 {
                return Err(BootControlError::GenerationGap);
            }
            let (generation, payload) = if generation_a > generation_b {
                (generation_a, payload_a)
            } else {
                (generation_b, payload_b)
            };
            Ok(SelectedRecord {
                generation,
                payload,
                degraded: true,
            })
        }
    }
}

fn validate_generation(generation: u64) -> Result<(), BootControlError> {
    if generation == 0 {
        return Err(BootControlError::InvalidGeneration);
    }
    Ok(())
}

fn validate_slot(slot: &SlotIdentity) -> Result<(), BootControlError> {
    if slot.sequence == 0
        || !valid_hex(&slot.manifest_sha256, 64)
        || !valid_layout_id(&slot.layout_id)
    {
        return Err(BootControlError::InvalidJournal);
    }
    Ok(())
}

fn validate_journal(journal: &SystemJournal) -> Result<(), BootControlError> {
    if !valid_layout_id(&journal.layout_id) || journal.floor_sequence == 0 {
        return Err(BootControlError::InvalidJournal);
    }
    validate_slot(&journal.confirmed)?;
    if journal.confirmed.layout_id != journal.layout_id
        || journal.confirmed.sequence != journal.floor_sequence
    {
        return Err(BootControlError::InvalidJournal);
    }
    let Some(pending) = journal.pending.as_ref() else {
        return Ok(());
    };
    validate_pending_journal(pending, journal)
}

fn validate_pending_journal(
    pending: &PendingSystemUpdate,
    journal: &SystemJournal,
) -> Result<(), BootControlError> {
    if !valid_hex(&pending.operation_id, 32)
        || pending.previous != journal.confirmed
        || pending.candidate.slot == pending.previous.slot
        || pending.candidate.sequence <= journal.floor_sequence
        || pending.candidate.layout_id != journal.layout_id
        || !(1..=3).contains(&pending.initial_attempts)
        || pending.remaining_attempts > pending.initial_attempts
    {
        return Err(BootControlError::InvalidJournal);
    }
    validate_slot(&pending.candidate)?;
    let boot_material = (
        pending.boot_id.as_deref(),
        pending.health_binding.as_deref(),
        pending.deadline_monotonic_ms,
    );
    let boot_material_absent = matches!(boot_material, (None, None, None));
    let boot_material_valid = matches!(
        boot_material,
        (Some(boot_id), Some(binding), Some(deadline))
            if valid_hex(boot_id, 32) && valid_hex(binding, 64) && deadline > 0
    );
    match pending.phase {
        JournalPhase::WritePrepared | JournalPhase::CandidateReady | JournalPhase::BootPrepared => {
            if pending.selector_generation.is_some()
                || pending.remaining_attempts != pending.initial_attempts
                || !boot_material_absent
            {
                return Err(BootControlError::InvalidJournal);
            }
        }
        JournalPhase::BootArmed => {
            if pending.selector_generation.is_none()
                || pending.remaining_attempts != pending.initial_attempts
                || !boot_material_absent
            {
                return Err(BootControlError::InvalidJournal);
            }
        }
        JournalPhase::AwaitingHealth | JournalPhase::ConfirmationPrepared => {
            if pending.selector_generation.is_none()
                || pending.remaining_attempts >= pending.initial_attempts
                || !boot_material_valid
            {
                return Err(BootControlError::InvalidJournal);
            }
        }
        JournalPhase::RollbackPrepared => {
            if pending.selector_generation.is_none()
                || !(boot_material_absent || boot_material_valid)
            {
                return Err(BootControlError::InvalidJournal);
            }
        }
    }
    Ok(())
}

fn validate_selector(selector: &BootSelector) -> Result<(), BootControlError> {
    if !valid_layout_id(&selector.layout_id) || !valid_hex(&selector.confirmed_manifest_sha256, 64)
    {
        return Err(BootControlError::InvalidSelector);
    }
    let Some(pending) = selector.pending.as_ref() else {
        return Ok(());
    };
    if !valid_hex(&pending.operation_id, 32)
        || pending.previous_slot != selector.confirmed_slot
        || pending.candidate_slot == pending.previous_slot
        || !valid_hex(&pending.candidate_manifest_sha256, 64)
        || !(1..=3).contains(&pending.initial_attempts)
        || pending.remaining_attempts > pending.initial_attempts
    {
        return Err(BootControlError::InvalidSelector);
    }
    match pending.state {
        SelectorPendingState::ArmedCandidate => {
            if pending.remaining_attempts != pending.initial_attempts {
                return Err(BootControlError::InvalidSelector);
            }
        }
        SelectorPendingState::CandidateBooted => {
            if pending.remaining_attempts >= pending.initial_attempts {
                return Err(BootControlError::InvalidSelector);
            }
        }
        SelectorPendingState::FallbackPrevious => {}
    }
    Ok(())
}

fn require_confirmed_previous(
    selector: &BootSelector,
    pending: &PendingSystemUpdate,
) -> Result<(), BootControlError> {
    if selector.pending.is_some() || !selector_matches_slot(selector, &pending.previous) {
        return Err(BootControlError::StateMismatch);
    }
    Ok(())
}

fn require_selector_pending<'a>(
    selector: &'a BootSelector,
    pending: &PendingSystemUpdate,
) -> Result<&'a PendingSelector, BootControlError> {
    let selector_pending = selector
        .pending
        .as_ref()
        .ok_or(BootControlError::StateMismatch)?;
    require_matching_pending(pending, selector_pending)?;
    Ok(selector_pending)
}

fn require_matching_pending(
    journal: &PendingSystemUpdate,
    selector: &PendingSelector,
) -> Result<(), BootControlError> {
    if journal.operation_id != selector.operation_id
        || journal.previous.slot != selector.previous_slot
        || journal.candidate.slot != selector.candidate_slot
        || journal.candidate.manifest_sha256 != selector.candidate_manifest_sha256
        || journal.initial_attempts != selector.initial_attempts
    {
        return Err(BootControlError::StateMismatch);
    }
    if selector.remaining_attempts > journal.remaining_attempts {
        return Err(BootControlError::AttemptCounterIncreased);
    }
    Ok(())
}

fn selector_matches_slot(selector: &BootSelector, slot: &SlotIdentity) -> bool {
    selector.confirmed_slot == slot.slot
        && selector.confirmed_manifest_sha256 == slot.manifest_sha256
}

fn valid_layout_id(value: &str) -> bool {
    (8..=64).contains(&value.len())
        && value.starts_with("layout-")
        && !value.ends_with('.')
        && !value.contains("..")
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && !value.bytes().all(|byte| byte == b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    value
}
