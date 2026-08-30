use std::collections::BTreeSet;

use bli_kvm_update_stager::StagedApplication;
use serde::{Deserialize, Serialize};

use crate::{RollbackReason, UpdateManagerError};

pub(crate) const JOURNAL_SCHEMA_VERSION: u32 = 1;
pub(crate) const MAX_HISTORY_RECORDS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SlotReference {
    pub(crate) slot_id: String,
    pub(crate) sequence: u64,
    pub(crate) manifest_sha256: String,
}

impl SlotReference {
    pub(crate) fn from_staged(staged: &StagedApplication) -> Self {
        Self {
            slot_id: staged.slot_id.clone(),
            sequence: staged.manifest.sequence,
            manifest_sha256: staged.manifest_sha256.clone(),
        }
    }

    pub(crate) fn matches_staged(&self, staged: &StagedApplication) -> bool {
        self == &Self::from_staged(staged)
    }

    fn validate(&self) -> bool {
        self.sequence != 0
            && valid_digest(&self.manifest_sha256)
            && self.slot_id == format!("app-{:020}-{}", self.sequence, self.manifest_sha256)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PendingPhase {
    Prepared,
    AwaitingHealth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PendingActivation {
    pub(crate) operation_id: String,
    pub(crate) health_binding: String,
    pub(crate) boot_id: String,
    pub(crate) prepared_monotonic_ms: u64,
    pub(crate) deadline_monotonic_ms: u64,
    pub(crate) previous: SlotReference,
    pub(crate) target: SlotReference,
    pub(crate) phase: PendingPhase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum OperationOutcome {
    Pending,
    Confirmed,
    RolledBack { reason: RollbackReason },
    Noop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OperationRecord {
    pub(crate) operation_id: String,
    pub(crate) target_slot_id: String,
    pub(crate) health_binding: String,
    pub(crate) outcome: OperationOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct JournalDocument {
    pub(crate) schema_version: u32,
    pub(crate) generation: u64,
    pub(crate) floor_sequence: u64,
    pub(crate) active: SlotReference,
    pub(crate) confirmed: SlotReference,
    pub(crate) previous: Option<SlotReference>,
    pub(crate) pending: Option<PendingActivation>,
    pub(crate) history: Vec<OperationRecord>,
    pub(crate) last_rollback: Option<RollbackReason>,
}

impl JournalDocument {
    pub(crate) fn initial(active: SlotReference) -> Self {
        Self {
            schema_version: JOURNAL_SCHEMA_VERSION,
            generation: 1,
            floor_sequence: active.sequence,
            active: active.clone(),
            confirmed: active,
            previous: None,
            pending: None,
            history: Vec::new(),
            last_rollback: None,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), UpdateManagerError> {
        if self.schema_version != JOURNAL_SCHEMA_VERSION
            || self.generation == 0
            || !self.active.validate()
            || !self.confirmed.validate()
            || self.floor_sequence != self.confirmed.sequence
            || self.previous.as_ref().is_some_and(|slot| !slot.validate())
            || self.history.len() > MAX_HISTORY_RECORDS
        {
            return Err(UpdateManagerError::InvalidJournal);
        }

        let mut operation_ids = BTreeSet::new();
        for record in &self.history {
            if !valid_nonzero_hex(&record.operation_id, 32)
                || !valid_nonzero_hex(&record.health_binding, 64)
                || !valid_slot_id(&record.target_slot_id)
                || !operation_ids.insert(record.operation_id.as_str())
            {
                return Err(UpdateManagerError::InvalidJournal);
            }
        }

        match &self.pending {
            Some(pending) => {
                if !valid_nonzero_hex(&pending.operation_id, 32)
                    || !valid_nonzero_hex(&pending.health_binding, 64)
                    || !valid_nonzero_hex(&pending.boot_id, 32)
                    || pending.deadline_monotonic_ms <= pending.prepared_monotonic_ms
                    || !pending.previous.validate()
                    || !pending.target.validate()
                    || pending.previous.slot_id == pending.target.slot_id
                    || pending.previous != self.confirmed
                    || pending.target.sequence <= self.floor_sequence
                    || match pending.phase {
                        PendingPhase::Prepared => self.active != pending.previous,
                        PendingPhase::AwaitingHealth => {
                            self.active != pending.target
                                || self.previous.as_ref() != Some(&pending.previous)
                        }
                    }
                {
                    return Err(UpdateManagerError::InvalidJournal);
                }
                let matching = self.history.iter().filter(|record| {
                    record.operation_id == pending.operation_id
                        && record.target_slot_id == pending.target.slot_id
                        && record.health_binding == pending.health_binding
                        && record.outcome == OperationOutcome::Pending
                });
                if matching.count() != 1 {
                    return Err(UpdateManagerError::InvalidJournal);
                }
            }
            None => {
                if self.active != self.confirmed
                    || self
                        .history
                        .iter()
                        .any(|record| record.outcome == OperationOutcome::Pending)
                {
                    return Err(UpdateManagerError::InvalidJournal);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn advance_generation(&mut self) -> Result<u64, UpdateManagerError> {
        let previous = self.generation;
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(UpdateManagerError::GenerationExhausted)?;
        Ok(previous)
    }

    pub(crate) fn find_record(&self, operation_id: &str) -> Option<&OperationRecord> {
        self.history
            .iter()
            .find(|record| record.operation_id == operation_id)
    }

    pub(crate) fn begin_operation(
        &mut self,
        operation_id: String,
        target_slot_id: String,
        health_binding: String,
    ) -> Result<(), UpdateManagerError> {
        if self.history.len() >= MAX_HISTORY_RECORDS {
            return Err(UpdateManagerError::HistoryFull);
        }
        self.history.push(OperationRecord {
            operation_id,
            target_slot_id,
            health_binding,
            outcome: OperationOutcome::Pending,
        });
        Ok(())
    }

    pub(crate) fn complete_operation(
        &mut self,
        operation_id: &str,
        outcome: OperationOutcome,
    ) -> Result<(), UpdateManagerError> {
        let record = self
            .history
            .iter_mut()
            .find(|record| record.operation_id == operation_id)
            .ok_or(UpdateManagerError::InvalidJournal)?;
        if record.outcome != OperationOutcome::Pending {
            return Err(UpdateManagerError::InvalidJournal);
        }
        record.outcome = outcome;
        Ok(())
    }
}

pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    value
}

pub(crate) fn valid_slot_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 89 || !bytes.starts_with(b"app-") || bytes[24] != b'-' {
        return false;
    }
    let sequence = &bytes[4..24];
    let digest = &bytes[25..];
    if !sequence.iter().all(u8::is_ascii_digit) || !valid_digest_bytes(digest) {
        return false;
    }
    let sequence = std::str::from_utf8(sequence).expect("ASCII digits are valid UTF-8");
    let Ok(sequence_number) = sequence.parse::<u64>() else {
        return false;
    };
    sequence_number != 0 && format!("{sequence_number:020}") == sequence
}

fn valid_digest(value: &str) -> bool {
    valid_digest_bytes(value.as_bytes())
}

fn valid_digest_bytes(bytes: &[u8]) -> bool {
    bytes.len() == 64
        && !bytes.iter().all(|byte| *byte == b'0')
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn valid_nonzero_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && !value.bytes().all(|byte| byte == b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
