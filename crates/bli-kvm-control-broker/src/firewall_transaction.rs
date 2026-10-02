//! Transaction ordering for native firewall mode changes.
//! The executor must use owner-scoped atomic rule replacement and obtain mode
//! from verified kernel rules, not merely its last requested value.

use crate::firewall_confirmation::{
    ConfirmationError, PendingChange, PendingJournal, PendingStore,
};
use crate::firewall_rules::FirewallMode;

pub trait ModeExecutor {
    fn preflight(&mut self, _mode: FirewallMode) -> Result<(), String> {
        Ok(())
    }
    fn admit_change(&self) -> Result<(), String> {
        Ok(())
    }
    fn observe(&mut self) -> Result<FirewallMode, String>;
    /// Configuration inputs must stay stable throughout a pending transaction,
    /// including across recovery, so previous mode regenerates previous policy.
    fn apply(&mut self, mode: FirewallMode) -> Result<(), String>;
    /// Replaces rules that an earlier build wrote for the mode in force by
    /// this build's rules for the same mode. The mode it did that for.
    fn refresh(&mut self) -> Result<Option<FirewallMode>, String> {
        Ok(None)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum TransactionError {
    ConfigurationUnavailable,
    Journal(ConfirmationError),
    Backend,
    ReadBackMismatch,
    RecoveryRequired,
}

impl From<ConfirmationError> for TransactionError {
    fn from(error: ConfirmationError) -> Self {
        Self::Journal(error)
    }
}

pub struct FirewallTransaction<S: PendingStore, E: ModeExecutor> {
    journal: PendingJournal<S>,
    executor: E,
    boot_id: [u8; 16],
    recovering: bool,
}

impl<S: PendingStore, E: ModeExecutor> FirewallTransaction<S, E> {
    pub fn open(store: S, executor: E, boot_id: [u8; 16]) -> Result<Self, TransactionError> {
        if boot_id == [0; 16] {
            return Err(ConfirmationError::InvalidIdentity.into());
        }
        let journal = PendingJournal::open(store)?;
        let recovering = journal.pending().is_some();
        Ok(Self {
            journal,
            executor,
            boot_id,
            recovering,
        })
    }

    pub fn pending(&self) -> Option<&PendingChange> {
        self.journal.pending()
    }

    pub fn current_mode(&mut self) -> Result<FirewallMode, TransactionError> {
        self.executor
            .observe()
            .map_err(|_| TransactionError::Backend)
    }

    pub fn status(
        &mut self,
        now_ms: u64,
    ) -> Result<bli_kvm_core::FirewallModeStatus, TransactionError> {
        let mode = self.current_mode()?;
        let pending = self.journal.pending();
        Ok(bli_kvm_core::FirewallModeStatus {
            mode,
            pending_operation_id: pending.map(PendingChange::operation_id),
            confirmation_deadline_unix_ms: pending.map(PendingChange::deadline_ms),
            recovery_required: self.recovering
                || pending
                    .is_some_and(|pending| !pending.awaiting_confirmation(self.boot_id, now_ms)),
            configuration_ready: self.executor.admit_change().is_ok(),
        })
    }

    pub fn request(
        &mut self,
        operation_id: [u8; 16],
        session_binding: [u8; 32],
        requested: FirewallMode,
        now_ms: u64,
    ) -> Result<(), TransactionError> {
        if self.recovering {
            return Err(TransactionError::RecoveryRequired);
        }
        if self.journal.pending().is_some() {
            return Err(ConfirmationError::PendingExists.into());
        }
        self.executor
            .admit_change()
            .map_err(|_| TransactionError::ConfigurationUnavailable)?;
        if operation_id == [0; 16] || session_binding == [0; 32] {
            return Err(ConfirmationError::InvalidIdentity.into());
        }
        let previous = self
            .executor
            .observe()
            .map_err(|_| TransactionError::Backend)?;
        if requested == previous {
            return Ok(());
        }
        self.executor
            .preflight(requested)
            .map_err(|_| TransactionError::ConfigurationUnavailable)?;
        let pending = PendingChange::prepare(
            operation_id,
            session_binding,
            self.boot_id,
            previous,
            requested,
            now_ms,
        )?;
        self.journal.stage(pending)?;
        if let Err(error) = self.apply_verified(requested) {
            // Failed rollback leaves the journal intact for the autonomous tick.
            let _ = self.rollback(operation_id);
            return Err(error);
        }
        if let Err(error) = self.journal.mark_applied(operation_id) {
            let _ = self.rollback(operation_id);
            return Err(error.into());
        }
        if !requested.requires_confirmation() {
            self.journal
                .confirm(operation_id, session_binding, self.boot_id, now_ms)?;
        }
        Ok(())
    }

    pub fn confirm(
        &mut self,
        operation_id: [u8; 16],
        session_binding: [u8; 32],
        now_ms: u64,
    ) -> Result<(), TransactionError> {
        if self.recovering {
            return Err(TransactionError::RecoveryRequired);
        }
        let pending = self.journal.pending().ok_or(ConfirmationError::NoPending)?;
        pending.can_confirm(operation_id, session_binding, self.boot_id, now_ms)?;
        if self
            .executor
            .observe()
            .map_err(|_| TransactionError::Backend)?
            != pending.requested_mode()
        {
            let _ = self.rollback(operation_id);
            return Err(TransactionError::ReadBackMismatch);
        }
        self.journal
            .confirm(operation_id, session_binding, self.boot_id, now_ms)?;
        Ok(())
    }

    /// Re-applies a mode the operator confirmed before a reboot, when the
    /// kernel holds no owned table (rules do not survive a reboot). The change
    /// is journaled like any other, so a crash during the restore rolls back to
    /// the open baseline; the HTTPS preflight is skipped because the web front
    /// may still be starting and the mode was confirmed with it running.
    /// Returns whether a restore happened.
    pub fn restore(&mut self, mode: FirewallMode, now_ms: u64) -> Result<bool, TransactionError> {
        if self.recovering || self.journal.pending().is_some() || mode == FirewallMode::Baseline {
            return Ok(false);
        }
        self.executor
            .admit_change()
            .map_err(|_| TransactionError::ConfigurationUnavailable)?;
        if self.current_mode()? != FirewallMode::Baseline {
            return Ok(false);
        }
        let mut operation_id = self.boot_id;
        operation_id[0] ^= 0x5a;
        let session_binding = [0xa5; 32];
        let pending = PendingChange::prepare(
            operation_id,
            session_binding,
            self.boot_id,
            FirewallMode::Baseline,
            mode,
            now_ms,
        )?;
        self.journal.stage(pending)?;
        if let Err(error) = self.apply_verified(mode) {
            let _ = self.rollback(operation_id);
            return Err(error);
        }
        self.journal.mark_applied(operation_id)?;
        self.journal
            .confirm(operation_id, session_binding, self.boot_id, now_ms)?;
        Ok(true)
    }

    /// After the owner was updated: the mode in force stays, but its rules
    /// become this build's. Not while a change waits for its confirmation or
    /// its rollback, which must find the rules they left.
    pub fn refresh(&mut self) -> Result<Option<FirewallMode>, TransactionError> {
        if self.recovering || self.journal.pending().is_some() {
            return Ok(None);
        }
        self.executor
            .refresh()
            .map_err(|_| TransactionError::Backend)
    }

    /// Invoke on the owner's independent timer, not from browser traffic only.
    pub fn tick(&mut self, now_ms: u64) -> Result<(), TransactionError> {
        let Some(pending) = self.journal.pending() else {
            self.recovering = false;
            return Ok(());
        };
        if self.recovering || pending.rollback_due(self.boot_id, now_ms) {
            let operation_id = pending.operation_id();
            self.rollback(operation_id)?;
            self.recovering = false;
        }
        Ok(())
    }

    fn apply_verified(&mut self, mode: FirewallMode) -> Result<(), TransactionError> {
        self.executor
            .apply(mode)
            .map_err(|_| TransactionError::Backend)?;
        if self
            .executor
            .observe()
            .map_err(|_| TransactionError::Backend)?
            != mode
        {
            return Err(TransactionError::ReadBackMismatch);
        }
        Ok(())
    }

    fn rollback(&mut self, operation_id: [u8; 16]) -> Result<(), TransactionError> {
        let previous = self.journal.begin_rollback(operation_id)?;
        self.apply_verified(previous)?;
        self.journal.finish_rollback(operation_id)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Store(Arc<Mutex<Option<String>>>);
    impl PendingStore for Store {
        fn load(&mut self) -> Result<Option<String>, String> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&mut self, record: &str) -> Result<(), String> {
            *self.0.lock().unwrap() = Some(record.into());
            Ok(())
        }
        fn clear(&mut self) -> Result<(), String> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }
    struct Executor {
        fail_preflight: bool,
        current: FirewallMode,
        fail_rollback: bool,
        mismatch: bool,
        store: Store,
        calls: Vec<FirewallMode>,
        refreshed: u32,
    }
    impl ModeExecutor for Executor {
        fn preflight(&mut self, _mode: FirewallMode) -> Result<(), String> {
            if self.fail_preflight {
                Err("HTTPS unavailable".into())
            } else {
                Ok(())
            }
        }
        fn observe(&mut self) -> Result<FirewallMode, String> {
            Ok(self.current)
        }
        fn refresh(&mut self) -> Result<Option<FirewallMode>, String> {
            self.refreshed += 1;
            Ok(Some(self.current))
        }
        fn apply(&mut self, mode: FirewallMode) -> Result<(), String> {
            assert!(
                self.store.0.lock().unwrap().is_some(),
                "kernel write preceded journal"
            );
            self.calls.push(mode);
            if self.fail_rollback && mode == FirewallMode::Moderate {
                return Err("offline".into());
            }
            if !self.mismatch || mode == FirewallMode::Moderate {
                self.current = mode;
            }
            Ok(())
        }
    }
    fn fixture() -> FirewallTransaction<Store, Executor> {
        let store = Store::default();
        let executor = Executor {
            fail_preflight: false,
            current: FirewallMode::Moderate,
            fail_rollback: false,
            mismatch: false,
            store: store.clone(),
            calls: vec![],
            refreshed: 0,
        };
        FirewallTransaction::open(store, executor, [3; 16]).unwrap()
    }

    #[test]
    fn a_confirmed_mode_is_restored_only_over_an_empty_kernel_state() {
        let mut tx = fixture();
        tx.executor.current = FirewallMode::Baseline;
        tx.executor.fail_preflight = true;
        assert_eq!(tx.restore(FirewallMode::Restricted, 100), Ok(true));
        assert_eq!(tx.executor.current, FirewallMode::Restricted);
        assert!(tx.pending().is_none());
        // Nothing to do over an owned table, a pending change or for baseline.
        assert_eq!(tx.restore(FirewallMode::Paranoid, 200), Ok(false));
        let mut tx = fixture();
        tx.executor.current = FirewallMode::Baseline;
        assert_eq!(tx.restore(FirewallMode::Baseline, 100), Ok(false));
        assert!(tx.executor.calls.is_empty());
        tx.request([1; 16], [2; 32], FirewallMode::Paranoid, 100)
            .unwrap();
        assert_eq!(tx.restore(FirewallMode::Moderate, 200), Ok(false));
    }

    #[test]
    fn rules_are_brought_up_to_date_only_while_no_change_is_pending() {
        let mut tx = fixture();
        assert_eq!(tx.refresh(), Ok(Some(FirewallMode::Moderate)));
        assert_eq!(tx.executor.refreshed, 1);
        // A change that waits for its confirmation: its rollback must find
        // the rules it left.
        tx.request([1; 16], [2; 32], FirewallMode::Restricted, 100)
            .unwrap();
        assert!(tx.pending().is_some());
        assert_eq!(tx.refresh(), Ok(None));
        assert_eq!(tx.executor.refreshed, 1);
        tx.confirm([1; 16], [2; 32], 200).unwrap();
        assert_eq!(tx.refresh(), Ok(Some(FirewallMode::Restricted)));
        assert_eq!(tx.executor.refreshed, 2);
    }

    #[test]
    fn a_failed_restore_returns_to_the_open_baseline() {
        let mut tx = fixture();
        tx.executor.current = FirewallMode::Baseline;
        tx.executor.mismatch = true;
        assert_eq!(
            tx.restore(FirewallMode::Paranoid, 100),
            Err(TransactionError::ReadBackMismatch)
        );
        assert!(tx.pending().is_none());
    }

    #[test]
    fn failed_preflight_precedes_journal_and_kernel_changes() {
        let mut tx = fixture();
        tx.executor.fail_preflight = true;
        assert_eq!(
            tx.request([1; 16], [2; 32], FirewallMode::Paranoid, 100),
            Err(TransactionError::ConfigurationUnavailable)
        );
        assert!(tx.pending().is_none());
        assert!(tx.executor.store.0.lock().unwrap().is_none());
        assert!(tx.executor.calls.is_empty());
    }

    #[test]
    fn nonrestrictive_changes_are_journaled_then_committed_without_user_confirmation() {
        let mut tx = fixture();
        tx.request([1; 16], [2; 32], FirewallMode::Baseline, 100)
            .unwrap();
        assert_eq!(tx.executor.current, FirewallMode::Baseline);
        assert!(tx.pending().is_none());
    }

    #[test]
    fn timeout_retries_failed_rollback_without_losing_record() {
        let mut tx = fixture();
        tx.request([1; 16], [2; 32], FirewallMode::Restricted, 100)
            .unwrap();
        let pending = tx.status(101).unwrap();
        assert_eq!(pending.pending_operation_id, Some([1; 16]));
        assert_eq!(pending.confirmation_deadline_unix_ms, Some(60_100));
        assert!(!pending.recovery_required);
        assert!(tx.status(60_100).unwrap().recovery_required);
        tx.executor.fail_rollback = true;
        assert_eq!(tx.tick(60_100), Err(TransactionError::Backend));
        assert!(tx.pending().is_some());
        assert!(tx.confirm([1; 16], [2; 32], 101).is_err());
        tx.executor.fail_rollback = false;
        tx.tick(60_101).unwrap();
        assert_eq!(tx.executor.current, FirewallMode::Moderate);
        assert!(tx.pending().is_none());
        assert_eq!(tx.status(60_101).unwrap().pending_operation_id, None);
    }

    #[test]
    fn confirmation_requires_identity_and_readback_and_prevents_late_rollback() {
        let mut tx = fixture();
        tx.request([1; 16], [2; 32], FirewallMode::Paranoid, 100)
            .unwrap();
        assert!(tx.confirm([9; 16], [2; 32], 101).is_err());
        assert_eq!(
            tx.request([4; 16], [2; 32], FirewallMode::Moderate, 101),
            Err(ConfirmationError::PendingExists.into())
        );
        tx.confirm([1; 16], [2; 32], 101).unwrap();
        tx.tick(90_000).unwrap();
        assert_eq!(tx.executor.current, FirewallMode::Paranoid);
        assert_eq!(tx.executor.calls, vec![FirewallMode::Paranoid]);
    }

    #[test]
    fn mismatched_apply_rolls_back_immediately_and_restart_blocks_confirmation() {
        let mut tx = fixture();
        tx.executor.mismatch = true;
        assert_eq!(
            tx.request([1; 16], [2; 32], FirewallMode::Restricted, 100),
            Err(TransactionError::ReadBackMismatch)
        );
        assert!(tx.pending().is_none());
        assert_eq!(
            tx.executor.calls,
            vec![FirewallMode::Restricted, FirewallMode::Moderate]
        );
        tx.executor.mismatch = false;
        tx.request([4; 16], [2; 32], FirewallMode::Restricted, 200)
            .unwrap();
        let store = tx.executor.store.clone();
        let mut reopened = FirewallTransaction::open(store, tx.executor, [3; 16]).unwrap();
        assert_eq!(
            reopened.confirm([4; 16], [2; 32], 201),
            Err(TransactionError::RecoveryRequired)
        );
        reopened.tick(201).unwrap();
        assert_eq!(reopened.executor.current, FirewallMode::Moderate);
        assert!(reopened.pending().is_none());
    }
}
