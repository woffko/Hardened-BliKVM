//! Privileged system-action handling: reboot and shutdown.
//!
//! The board owner keeps the privilege and the journal. This module turns a
//! confirmed wire request into a prepared, journaled action and dispatches it
//! through a runner, which is the maintenance dispatcher on the device and a
//! mock in the host tests. The capability is disabled unless the reviewed
//! profile explicitly arms it.

use std::path::PathBuf;

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};
use bli_kvm_core::{SystemActionKind, SystemActionWireRequest};
use bli_kvm_system_action_policy::{
    GuardState, SystemAction, SystemActionDecision, SystemActionDispatchDecision,
    SystemActionEligibility, SystemActionEnvironment, SystemActionPolicyEngine,
    SystemActionRejection, SystemActionRequest,
};

/// The action runner the prepared action is handed to.
pub trait ActionRunner: Send {
    /// Perform the action. Returning an error is a backend rejection, never a
    /// silent success; the journal keeps the intent either way.
    fn dispatch(&mut self, action: SystemAction) -> Result<(), String>;
}

impl ActionRunner for Box<dyn ActionRunner> {
    fn dispatch(&mut self, action: SystemAction) -> Result<(), String> {
        (**self).dispatch(action)
    }
}

/// Runner that executes one reviewed, argument-free command per action.
///
/// The command is an operator-supplied absolute path (a reviewed wrapper, not
/// a shell line); nothing is executed unless both paths are configured, and no
/// argument is ever passed, so a request cannot inject one.
#[derive(Debug, Default)]
pub struct ExecRunner {
    reboot: Option<PathBuf>,
    shutdown: Option<PathBuf>,
}

impl ExecRunner {
    pub fn new(reboot: Option<PathBuf>, shutdown: Option<PathBuf>) -> Self {
        Self { reboot, shutdown }
    }

    pub fn is_configured(&self) -> bool {
        self.reboot.is_some() && self.shutdown.is_some()
    }
}

impl ActionRunner for ExecRunner {
    fn dispatch(&mut self, action: SystemAction) -> Result<(), String> {
        let path = match action {
            SystemAction::Reboot => self.reboot.as_ref(),
            SystemAction::Shutdown => self.shutdown.as_ref(),
        };
        let Some(path) = path else {
            return Err("the system action command is not configured".into());
        };
        if !path.is_absolute() {
            return Err("the system action command must be absolute".into());
        }
        let status = std::process::Command::new(path)
            .status()
            .map_err(|error| error.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("the system action command exited with {status}"))
        }
    }
}

/// A runner that refuses everything; the default until a profile arms one.
#[derive(Debug, Default)]
pub struct RefusingRunner;

impl ActionRunner for RefusingRunner {
    fn dispatch(&mut self, _action: SystemAction) -> Result<(), String> {
        Err("system action dispatch is not configured".into())
    }
}

pub struct SystemActionState<R: ActionRunner> {
    policy: SystemActionPolicyEngine,
    runner: R,
    capability_enabled: bool,
    backend_ready: bool,
    /// Origin boot binding: the running boot this owner belongs to.
    boot_binding: [u8; 16],
    /// Persisted journal, when the profile keeps one. The state is loaded at
    /// construction and written back after every phase change, so a restart or
    /// a reboot cannot lose a prepared action.
    store: Option<bli_kvm_system_action_store::SystemActionStore>,
    generation: u64,
}

impl<R: ActionRunner> SystemActionState<R> {
    pub fn new(
        runner: R,
        capability_enabled: bool,
        backend_ready: bool,
        boot_binding: [u8; 16],
    ) -> Self {
        Self {
            policy: SystemActionPolicyEngine::new(),
            runner,
            capability_enabled,
            backend_ready,
            boot_binding,
            store: None,
            generation: 0,
        }
    }

    /// Attach the persisted journal and restore the state it holds.
    ///
    /// An empty store keeps a fresh engine; a stored state is restored, so a
    /// prepared or dispatching action survives an owner restart and a reboot.
    pub fn with_store(
        &mut self,
        store: bli_kvm_system_action_store::SystemActionStore,
    ) -> Result<(), &'static str> {
        match store.load(1) {
            Ok(selected) => {
                self.policy = SystemActionPolicyEngine::restore(selected.state())
                    .map_err(|_| "the stored system-action state is invalid")?;
                self.generation = selected.generation();
            }
            Err(bli_kvm_system_action_store::SystemActionStoreError::Journal(
                bli_kvm_system_action_store::JournalError::NoValidCopy,
            )) => {
                // The store exists but holds no journal yet: start fresh and
                // let the first persist initialize it.
                self.generation = 0;
            }
            Err(_) => return Err("the system-action journal is unavailable"),
        }
        self.store = Some(store);
        Ok(())
    }

    /// Fail closed: a journal that cannot be trusted must not leave the
    /// capability enabled, because an unrecorded action could be dispatched and
    /// then never accepted after the reboot.
    pub fn disable_capability(&mut self) {
        self.capability_enabled = false;
    }

    fn persist(&mut self) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        if self.generation == 0 {
            if let Ok(persisted) = store.initialize(self.policy.snapshot()) {
                self.generation = persisted.generation;
            }
            return;
        }
        if let Ok(persisted) = store.update(self.generation, 1, self.policy.snapshot()) {
            self.generation = persisted.generation;
        }
    }

    pub fn capability_enabled(&self) -> bool {
        self.capability_enabled
    }

    pub fn pending_count(&self) -> usize {
        self.policy.pending_count()
    }

    /// Handle one confirmed system-action request.
    pub fn handle(
        &mut self,
        operation_id: &str,
        request: &SystemActionWireRequest,
        now_ms: u64,
    ) -> ControlResponse {
        if request.validate().is_err() {
            return reject(ControlRejectionCode::InvalidState);
        }
        let Some(operation_id) = hex_operation_id(operation_id) else {
            return reject(ControlRejectionCode::InvalidState);
        };
        let action = match request.action {
            SystemActionKind::Reboot => SystemAction::Reboot,
            SystemActionKind::Shutdown => SystemAction::Shutdown,
        };
        let Ok(prepared) = SystemActionRequest::new(
            operation_id,
            request.session_binding,
            request.confirmation_binding,
            request.confirmed_at_unix_ms,
            self.boot_binding,
            action,
        ) else {
            return reject(ControlRejectionCode::InvalidState);
        };
        let eligibility = SystemActionEligibility::reviewed(
            self.capability_enabled,
            self.backend_ready,
            true,
            true,
            true,
        );
        let guards = if self.capability_enabled {
            GuardState::Ready
        } else {
            GuardState::Unknown
        };
        let Ok(environment) =
            SystemActionEnvironment::new(self.boot_binding, now_ms, guards, guards, guards, guards)
        else {
            return reject(ControlRejectionCode::Internal);
        };

        match self.policy.prepare(prepared, eligibility, environment) {
            Ok(SystemActionDecision::PersistPrepared(ticket)) => {
                self.persist();
                match self
                    .policy
                    .record_dispatch_intent(ticket, eligibility, environment)
                {
                    Ok(SystemActionDispatchDecision::PersistDispatching(_intent)) => {
                        self.persist();
                        match self.runner.dispatch(action) {
                            Ok(()) => ControlResponse::Applied,
                            Err(reason) => {
                                eprintln!("system action dispatch failed: {reason}");
                                reject(ControlRejectionCode::BackendRejected)
                            }
                        }
                    }
                    Ok(SystemActionDispatchDecision::PersistRejected(reason)) => {
                        reject(rejection_code(reason))
                    }
                    Err(_) => reject(ControlRejectionCode::Internal),
                }
            }
            Ok(SystemActionDecision::ReplayPostBootAccepted) => ControlResponse::Applied,
            Ok(SystemActionDecision::ReplayRejected) => {
                reject(ControlRejectionCode::BackendRejected)
            }
            Ok(SystemActionDecision::Reject(reason)) => reject(rejection_code(reason)),
            Err(_) => reject(ControlRejectionCode::Internal),
        }
    }
}

fn reject(code: ControlRejectionCode) -> ControlResponse {
    ControlResponse::Rejected { code }
}

fn hex_operation_id(value: &str) -> Option<[u8; 16]> {
    if value.len() != 32 {
        return None;
    }
    let mut bytes = [0_u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let pair = value.get(index * 2..index * 2 + 2)?;
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    if bytes.iter().all(|byte| *byte == 0) {
        return None;
    }
    Some(bytes)
}

/// Map the policy rejection onto the fixed protocol codes.
fn rejection_code(reason: SystemActionRejection) -> ControlRejectionCode {
    match reason {
        SystemActionRejection::CapabilityDisabled => ControlRejectionCode::CapabilityDisabled,
        SystemActionRejection::BackendUnavailable
        | SystemActionRejection::RecoveryUnavailable
        | SystemActionRejection::PostBootObserverUnavailable
        | SystemActionRejection::PostBootHealthUnavailable
        | SystemActionRejection::JournalFull => ControlRejectionCode::BackendUnavailable,
        SystemActionRejection::LocalAdminRequired
        | SystemActionRejection::ReauthenticationRequired
        | SystemActionRejection::ConfirmationInvalid
        | SystemActionRejection::StartupGuardActive
        | SystemActionRejection::UpdateNotIdle
        | SystemActionRejection::StorageNotIdle => ControlRejectionCode::NotArmed,
        SystemActionRejection::BootBindingMismatch => ControlRejectionCode::InvalidState,
        SystemActionRejection::ConfirmationExpired => ControlRejectionCode::ConfirmationStale,
        SystemActionRejection::OperationIdConflict => ControlRejectionCode::Duplicate,
        SystemActionRejection::IndeterminatePending
        | SystemActionRejection::PostBootNotObserved => ControlRejectionCode::PendingUnreconciled,
        SystemActionRejection::CooldownActive => ControlRejectionCode::CooldownActive,
        SystemActionRejection::DispatchNotRecorded => ControlRejectionCode::Internal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct MockRunner {
        dispatched: Arc<Mutex<Vec<SystemAction>>>,
        fail: bool,
    }

    impl ActionRunner for MockRunner {
        fn dispatch(&mut self, action: SystemAction) -> Result<(), String> {
            if self.fail {
                return Err("mock failure".into());
            }
            self.dispatched.lock().unwrap().push(action);
            Ok(())
        }
    }

    fn request() -> SystemActionWireRequest {
        SystemActionWireRequest {
            action: SystemActionKind::Reboot,
            session_binding: 9,
            confirmation_binding: [0x11; 32],
            confirmed_at_unix_ms: 119_000,
        }
    }

    fn state(capability_enabled: bool) -> SystemActionState<MockRunner> {
        SystemActionState::new(MockRunner::default(), capability_enabled, true, [0x22; 16])
    }

    #[test]
    fn disabled_capability_is_rejected() {
        let mut state = state(false);
        let response = state.handle("a123456789abcdef0123456789abcdef", &request(), 120_000);
        assert_eq!(
            response,
            ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled
            }
        );
    }

    #[test]
    fn armed_capability_dispatches_once_and_replays() {
        let runner = MockRunner::default();
        let dispatched = Arc::clone(&runner.dispatched);
        let mut state = SystemActionState::new(runner, true, true, [0x22; 16]);
        let response = state.handle("a123456789abcdef0123456789abcdef", &request(), 120_000);
        assert_eq!(response, ControlResponse::Applied);
        assert_eq!(dispatched.lock().unwrap().len(), 1);

        // The same operation replays without a second dispatch.
        let replay = state.handle("a123456789abcdef0123456789abcdef", &request(), 121_000);
        assert!(matches!(
            replay,
            ControlResponse::Applied | ControlResponse::Rejected { .. }
        ));
        assert_eq!(dispatched.lock().unwrap().len(), 1);
    }

    #[test]
    fn stale_confirmation_and_bad_operation_ids_are_rejected() {
        let mut state = state(true);
        let mut stale = request();
        stale.confirmed_at_unix_ms = 1;
        let response = state.handle("a123456789abcdef0123456789abcdef", &stale, 10_000_000);
        assert_eq!(
            response,
            ControlResponse::Rejected {
                code: ControlRejectionCode::ConfirmationStale
            }
        );

        let response = state.handle("not-hex", &request(), 1_100);
        assert_eq!(
            response,
            ControlResponse::Rejected {
                code: ControlRejectionCode::InvalidState
            }
        );

        let mut invalid = request();
        invalid.confirmation_binding = [0; 32];
        let response = state.handle("b123456789abcdef0123456789abcdef", &invalid, 1_100);
        assert_eq!(
            response,
            ControlResponse::Rejected {
                code: ControlRejectionCode::InvalidState
            }
        );
    }

    #[test]
    fn the_journal_survives_a_restart() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("store");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        // Safety: the effective ids are read once for the store ownership.
        let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };

        let store = bli_kvm_system_action_store::SystemActionStore::open_at(root.clone(), uid, gid)
            .unwrap();
        {
            let mut first = SystemActionState::new(MockRunner::default(), true, true, [0x22; 16]);
            first.with_store(store).unwrap();
            assert_eq!(
                first.handle("a123456789abcdef0123456789abcdef", &request(), 120_000),
                ControlResponse::Applied
            );
        }

        // A fresh state reading the same store must see the recorded phase, so
        // an owner restart cannot silently lose the action.
        let store =
            bli_kvm_system_action_store::SystemActionStore::open_at(root, uid, gid).unwrap();
        let mut second = SystemActionState::new(MockRunner::default(), true, true, [0x22; 16]);
        second.with_store(store).unwrap();
        let replay = second.handle("a123456789abcdef0123456789abcdef", &request(), 121_000);
        assert_eq!(replay, ControlResponse::Applied);

        // A different action must stay blocked by the restored pending record.
        let mut next_request = request();
        next_request.confirmed_at_unix_ms = 120_000;
        let blocked = second.handle("b123456789abcdef0123456789abcdef", &next_request, 121_000);
        assert_eq!(
            blocked,
            ControlResponse::Rejected {
                code: ControlRejectionCode::PendingUnreconciled
            }
        );
    }

    #[test]
    fn exec_runner_runs_the_reviewed_command_only_when_configured() {
        let mut unconfigured = ExecRunner::default();
        assert!(!unconfigured.is_configured());
        assert!(unconfigured.dispatch(SystemAction::Reboot).is_err());

        let mut ok = ExecRunner::new(
            Some(PathBuf::from("/bin/true")),
            Some(PathBuf::from("/bin/true")),
        );
        assert!(ok.is_configured());
        assert_eq!(ok.dispatch(SystemAction::Reboot), Ok(()));
        assert_eq!(ok.dispatch(SystemAction::Shutdown), Ok(()));

        let mut failing = ExecRunner::new(
            Some(PathBuf::from("/bin/false")),
            Some(PathBuf::from("/bin/false")),
        );
        assert!(failing.dispatch(SystemAction::Reboot).is_err());

        let mut relative =
            ExecRunner::new(Some(PathBuf::from("true")), Some(PathBuf::from("true")));
        assert!(relative.dispatch(SystemAction::Shutdown).is_err());
    }

    #[test]
    fn failing_runner_is_a_backend_rejection() {
        let mut state = SystemActionState::new(
            MockRunner {
                fail: true,
                ..MockRunner::default()
            },
            true,
            true,
            [0x22; 16],
        );
        let response = state.handle("c123456789abcdef0123456789abcdef", &request(), 120_000);
        assert_eq!(
            response,
            ControlResponse::Rejected {
                code: ControlRejectionCode::BackendRejected
            }
        );
    }
}
