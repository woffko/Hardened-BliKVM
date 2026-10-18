use std::collections::BTreeSet;

use bli_kvm_update_stager::StagedApplication;
use serde::{Deserialize, Serialize};

use crate::{
    NativeRecoveryCompletion, NativeRollbackIntent, NativeRollbackPhase, RollbackReason,
    UpdateManagerError,
};

pub(crate) const JOURNAL_SCHEMA_VERSION: u32 = 1;
pub(crate) const NATIVE_JOURNAL_SCHEMA_VERSION: u32 = 2;
pub(crate) const NATIVE_LIFECYCLE_SCHEMA_VERSION: u32 = 3;
pub(crate) const MAX_HISTORY_RECORDS: usize = 128;
/// Finished records a rollover keeps, the newest (security review of
/// 2026-10-18, LOW-01).
pub(crate) const KEPT_FINISHED_RECORDS: usize = 64;

fn is_zero(value: &u64) -> bool {
    *value == 0
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) native_completion: Option<NativeRecoveryCompletion>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) native_rollback: Option<NativeRollbackIntent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) native_completion: Option<NativeRecoveryCompletion>,
    /// The generation at which older finished records were last dropped
    /// from a full history; 0 while none was. Written only once a rollover
    /// happened, so a journal that never filled keeps its bytes; a manager
    /// from before it refuses the field, as it would a new schema.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) pruned_through_generation: u64,
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
            native_rollback: None,
            native_completion: None,
            pruned_through_generation: 0,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), UpdateManagerError> {
        if ![
            JOURNAL_SCHEMA_VERSION,
            NATIVE_JOURNAL_SCHEMA_VERSION,
            NATIVE_LIFECYCLE_SCHEMA_VERSION,
        ]
        .contains(&self.schema_version)
            || (self.schema_version == JOURNAL_SCHEMA_VERSION
                && (self.native_rollback.is_some() || self.native_completion.is_some()))
            || (self.schema_version == NATIVE_JOURNAL_SCHEMA_VERSION
                && (self.native_rollback.is_some() == self.native_completion.is_some()))
            || (self.native_rollback.is_some() && self.native_completion.is_some())
            || self.generation == 0
            || !self.active.validate()
            || !self.confirmed.validate()
            || self.floor_sequence != self.confirmed.sequence
            || self.previous.as_ref().is_some_and(|slot| !slot.validate())
            || self.history.len() > MAX_HISTORY_RECORDS
            || self.pruned_through_generation > self.generation
        {
            return Err(UpdateManagerError::InvalidJournal);
        }
        if let Some(intent) = &self.native_rollback {
            let pending = self
                .pending
                .as_ref()
                .ok_or(UpdateManagerError::InvalidJournal)?;
            if intent.operation_id != pending.operation_id
                || intent.intent_generation == 0
                || intent.intent_generation > self.generation
                || (intent.reason == RollbackReason::HealthChallengeLost
                    && self.schema_version != NATIVE_LIFECYCLE_SCHEMA_VERSION)
            {
                return Err(UpdateManagerError::InvalidJournal);
            }
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
            if let Some(completion) = &record.native_completion {
                let intent = &completion.intent;
                if self.schema_version != NATIVE_LIFECYCLE_SCHEMA_VERSION
                    || intent.phase != NativeRollbackPhase::Restoring
                    || intent.intent_generation == 0
                    || intent.intent_generation >= completion.completion_generation
                    || completion.completion_generation >= self.generation
                    || !valid_slot_id(&completion.confirmed_slot_id)
                    || completion.confirmed_slot_id == completion.failed_target_slot_id
                    || intent.operation_id != record.operation_id
                    || completion.failed_target_slot_id != record.target_slot_id
                    || completion.health_binding != record.health_binding
                    || record.outcome
                        != (OperationOutcome::RolledBack {
                            reason: intent.reason,
                        })
                    || self
                        .native_completion
                        .as_ref()
                        .is_some_and(|current| current.intent.operation_id == intent.operation_id)
                {
                    return Err(UpdateManagerError::InvalidJournal);
                }
            }
        }

        if let Some(completion) = &self.native_completion {
            let intent = &completion.intent;
            if intent.phase != NativeRollbackPhase::Restoring
                || (intent.reason == RollbackReason::HealthChallengeLost
                    && self.schema_version != NATIVE_LIFECYCLE_SCHEMA_VERSION)
                || intent.intent_generation == 0
                || intent.intent_generation >= completion.completion_generation
                || completion.completion_generation != self.generation
                || completion.confirmed_slot_id != self.confirmed.slot_id
                || !valid_slot_id(&completion.failed_target_slot_id)
                || completion.failed_target_slot_id == completion.confirmed_slot_id
                || self.pending.is_some()
                || self.previous.is_some()
                || self.last_rollback != Some(intent.reason)
                || !self.history.iter().any(|record| {
                    record.operation_id == intent.operation_id
                        && record.target_slot_id == completion.failed_target_slot_id
                        && record.health_binding == completion.health_binding
                        && record.outcome
                            == OperationOutcome::RolledBack {
                                reason: intent.reason,
                            }
                })
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
                    || if self
                        .native_rollback
                        .as_ref()
                        .is_some_and(|intent| intent.phase == NativeRollbackPhase::Restoring)
                    {
                        self.active != pending.previous || self.previous.is_some()
                    } else {
                        match pending.phase {
                            PendingPhase::Prepared => self.active != pending.previous,
                            PendingPhase::AwaitingHealth => {
                                self.active != pending.target
                                    || self.previous.as_ref() != Some(&pending.previous)
                            }
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
                if matching.count() != 1
                    || self
                        .history
                        .iter()
                        .filter(|record| record.outcome == OperationOutcome::Pending)
                        .count()
                        != 1
                {
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

    /// Reserve serialization capacity for a complete native attempt, including
    /// failure recovery and receipt archival. No state is written here. Use the
    /// widest generation encoding and longest currently supported reason so
    /// decimal growth cannot consume the reserved space later.
    pub(crate) fn validate_native_activation_capacity(&self) -> Result<(), UpdateManagerError> {
        self.generation
            .checked_add(5)
            .ok_or(UpdateManagerError::GenerationExhausted)?;
        let pending = self
            .pending
            .as_ref()
            .ok_or(UpdateManagerError::InvalidJournal)?;
        if self.schema_version != NATIVE_LIFECYCLE_SCHEMA_VERSION
            || pending.phase != PendingPhase::Prepared
            || self.native_rollback.is_some()
            || self.native_completion.is_some()
        {
            return Err(UpdateManagerError::InvalidJournal);
        }
        let mut probe = self.clone();
        probe.generation = u64::MAX - 5;
        probe.last_rollback = Some(RollbackReason::ExplicitHealthFailure);
        let _ = crate::journal::encode_document(&probe)?;
        probe.generation += 1;
        probe.active = pending.target.clone();
        probe.previous = Some(pending.previous.clone());
        probe
            .pending
            .as_mut()
            .ok_or(UpdateManagerError::InvalidJournal)?
            .phase = PendingPhase::AwaitingHealth;
        let _ = crate::journal::encode_document(&probe)?;

        let mut confirmed = probe.clone();
        confirmed.generation += 1;
        confirmed.confirmed = pending.target.clone();
        confirmed.floor_sequence = pending.target.sequence;
        confirmed.pending = None;
        confirmed.last_rollback = None;
        confirmed.complete_operation(&pending.operation_id, OperationOutcome::Confirmed)?;
        let _ = crate::journal::encode_document(&confirmed)?;

        probe.generation += 1;
        let mut intent = NativeRollbackIntent {
            operation_id: pending.operation_id.clone(),
            intent_generation: probe.generation,
            reason: RollbackReason::ExplicitHealthFailure,
            phase: NativeRollbackPhase::RollbackPending,
        };
        probe.native_rollback = Some(intent.clone());
        let _ = crate::journal::encode_document(&probe)?;
        probe.generation += 1;
        probe.active = pending.previous.clone();
        probe.previous = None;
        intent.phase = NativeRollbackPhase::Restoring;
        probe.native_rollback = Some(intent.clone());
        let _ = crate::journal::encode_document(&probe)?;
        probe.generation += 1;
        probe.pending = None;
        probe.native_rollback = None;
        probe.complete_operation(
            &pending.operation_id,
            OperationOutcome::RolledBack {
                reason: intent.reason,
            },
        )?;
        let receipt = NativeRecoveryCompletion {
            intent,
            completion_generation: probe.generation,
            failed_target_slot_id: pending.target.slot_id.clone(),
            confirmed_slot_id: pending.previous.slot_id.clone(),
            health_binding: pending.health_binding.clone(),
        };
        probe.native_completion = Some(receipt.clone());
        let _ = crate::journal::encode_document(&probe)?;
        probe.generation += 1;
        probe.native_completion = None;
        probe
            .history
            .iter_mut()
            .find(|record| record.operation_id == pending.operation_id)
            .ok_or(UpdateManagerError::InvalidJournal)?
            .native_completion = Some(receipt);
        let _ = crate::journal::encode_document(&probe)?;
        Ok(())
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
            self.roll_over_history();
        }
        if self.history.len() >= MAX_HISTORY_RECORDS {
            return Err(UpdateManagerError::HistoryFull);
        }
        self.history.push(OperationRecord {
            operation_id,
            target_slot_id,
            health_binding,
            outcome: OperationOutcome::Pending,
            native_completion: None,
        });
        Ok(())
    }

    /// Makes room in a full history: every pending record and the one the
    /// current rollback receipt names stay, with the newest finished
    /// records; older finished ones go. Their retries need no answer from
    /// here: an admission is bound to the journal's exact generation, which
    /// has moved on since.
    fn roll_over_history(&mut self) {
        let receipt = self
            .native_completion
            .as_ref()
            .map(|completion| completion.intent.operation_id.clone());
        let mut finished_kept = 0_usize;
        let mut kept = vec![false; self.history.len()];
        for (index, record) in self.history.iter().enumerate().rev() {
            let needed = record.outcome == OperationOutcome::Pending
                || receipt.as_deref() == Some(record.operation_id.as_str());
            if needed {
                kept[index] = true;
            } else if finished_kept < KEPT_FINISHED_RECORDS {
                finished_kept += 1;
                kept[index] = true;
            }
        }
        let mut flags = kept.into_iter();
        self.history.retain(|_| flags.next().unwrap_or(true));
        self.pruned_through_generation = self.generation;
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
