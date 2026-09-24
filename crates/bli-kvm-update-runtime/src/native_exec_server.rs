//! One bounded session of the root execution helper.
//! The peer is authenticated by UID before any byte is read. Requests are the
//! closed `update-protocol::execution` variants; this layer maps them to fixed
//! units, the protected store and the selection ledger. It never opens the
//! update manager, the journal or health credentials.

use std::{
    fs::File,
    io::{self, Read, Write},
    net::SocketAddr,
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use bli_kvm_update_protocol::execution::{
    ExecutionErrorCode, ExecutionIdentity, ExecutionProbe, ExecutionRequest,
    ExecutionRequestEnvelope, ExecutionResponse, ExecutionResponseEnvelope, ExecutionStartUnit,
    ExecutionStopObservation, ExecutionUnit, MAX_EXECUTION_MESSAGE_BYTES,
    MAX_EXECUTION_SESSION_REQUESTS, decode_request, encode_response,
};
use bli_kvm_update_stager::{ApplicationStageError, VerifiedSlotFiles};

use crate::{
    board_readiness::BoardRequirements,
    composition::{ApplicationUnit, CompositionError, ServiceController, StopObservation},
    composition_systemd::SystemdController,
    native_execution::NativeExecutionStore,
    native_identity::{FILES, NativeStartUnit, SlotServer},
    native_selection::SelectionLedgerStore,
    process_identity::ProcessIdentity,
};

/// A session may not outlive one composition attempt plus admission.
pub const MAX_EXECUTION_SESSION: Duration = Duration::from_secs(90);

const MANAGED_UNITS: [ApplicationUnit; 7] = [
    ApplicationUnit::BoardServer,
    ApplicationUnit::CurrentServer,
    ApplicationUnit::CandidateServer,
    ApplicationUnit::BootstrapServer,
    ApplicationUnit::BoardDaemon,
    ApplicationUnit::VideoProxy,
    ApplicationUnit::Owner,
];

/// Privileged operations on the fixed managed units. Implementations bound
/// every call by the session deadline supplied at construction.
pub trait ExecutionBackend {
    /// Cancel queued jobs of every managed unit. Success means none remains.
    fn reconcile(&mut self) -> Result<(), CompositionError>;
    fn stop(&mut self, unit: ApplicationUnit) -> Result<(), CompositionError>;
    fn observe_stop(&mut self, unit: ApplicationUnit) -> Result<StopObservation, CompositionError>;
    fn validate_start(&self, unit: NativeStartUnit) -> Result<(), CompositionError>;
    fn start(&mut self, unit: NativeStartUnit) -> Result<(), CompositionError>;
    fn observe_identity(
        &self,
        unit: ApplicationUnit,
        expected: &File,
    ) -> Result<ProcessIdentity, CompositionError>;
    fn probe(
        &mut self,
        probe: ExecutionProbe,
        selected: &VerifiedSlotFiles,
    ) -> Result<(), CompositionError>;
}

/// Probe endpoints and requirements from the helper's protected configuration.
#[derive(Debug, Clone)]
pub struct ExecutionProbeConfig {
    pub owner_socket: PathBuf,
    pub video_socket: PathBuf,
    pub board_socket: PathBuf,
    pub web_endpoint: SocketAddr,
    pub board: BoardRequirements,
}

/// Production backend: root systemctl on the closed units plus the existing
/// read-only readiness probes. Each operation gets a fresh controller bounded
/// by the session deadline and the adapter's 45-second ceiling.
pub struct SystemdExecutionBackend {
    probes: ExecutionProbeConfig,
    deadline: Instant,
}

impl SystemdExecutionBackend {
    pub fn new(probes: ExecutionProbeConfig, deadline: Instant) -> Self {
        Self { probes, deadline }
    }

    fn controller(&self) -> Result<SystemdController, CompositionError> {
        let now = Instant::now();
        if now >= self.deadline {
            return Err(CompositionError::Deadline);
        }
        SystemdController::new(self.deadline.min(now + Duration::from_secs(45)))
    }
}

impl ExecutionBackend for SystemdExecutionBackend {
    fn reconcile(&mut self) -> Result<(), CompositionError> {
        let controller = self.controller()?;
        for unit in MANAGED_UNITS {
            controller.cancel_queued_job(unit)?;
        }
        Ok(())
    }

    fn stop(&mut self, unit: ApplicationUnit) -> Result<(), CompositionError> {
        self.controller()?.stop(unit)
    }

    fn observe_stop(&mut self, unit: ApplicationUnit) -> Result<StopObservation, CompositionError> {
        self.controller()?.observe_stop(unit)
    }

    fn validate_start(&self, unit: NativeStartUnit) -> Result<(), CompositionError> {
        self.controller()?.validate_native_start(unit)
    }

    fn start(&mut self, unit: NativeStartUnit) -> Result<(), CompositionError> {
        self.controller()?.start_native_unit(unit)
    }

    fn observe_identity(
        &self,
        unit: ApplicationUnit,
        expected: &File,
    ) -> Result<ProcessIdentity, CompositionError> {
        self.controller()?.observe_unit_identity(unit, expected)
    }

    fn probe(
        &mut self,
        probe: ExecutionProbe,
        selected: &VerifiedSlotFiles,
    ) -> Result<(), CompositionError> {
        let failed = |_| CompositionError::Controller;
        let pid = |unit: NativeStartUnit| -> Result<u32, CompositionError> {
            let file = selected
                .files()
                .get(executable_path(unit))
                .ok_or(CompositionError::IncompletePayload)?;
            Ok(self.observe_identity(unit.unit(), file)?.pid)
        };
        let deadline = self.deadline;
        match probe {
            ExecutionProbe::Owner => crate::owner_readiness::probe_readbacks(
                &self.probes.owner_socket,
                pid(NativeStartUnit::Owner)?,
                self.probes.board,
                deadline,
            )
            .map(drop)
            .map_err(failed),
            ExecutionProbe::VideoProxy => crate::proxy_readiness::probe_first_part(
                &self.probes.video_socket,
                pid(NativeStartUnit::VideoProxy)?,
                deadline,
            )
            .map(drop)
            .map_err(failed),
            ExecutionProbe::BoardDaemon => crate::board_readiness::probe(
                &self.probes.board_socket,
                pid(NativeStartUnit::BoardDaemon)?,
                self.probes.board,
                deadline,
            )
            .map_err(failed),
            ExecutionProbe::Web => tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| CompositionError::Controller)?
                .block_on(crate::web_probe::verify_tree(
                    &selected.slot().manifest,
                    self.probes.web_endpoint,
                    deadline,
                ))
                .map(drop)
                .map_err(failed),
        }
    }
}

pub(crate) const fn application_unit(unit: ExecutionUnit) -> ApplicationUnit {
    match unit {
        ExecutionUnit::BoardServer => ApplicationUnit::BoardServer,
        ExecutionUnit::CurrentServer => ApplicationUnit::CurrentServer,
        ExecutionUnit::CandidateServer => ApplicationUnit::CandidateServer,
        ExecutionUnit::BootstrapServer => ApplicationUnit::BootstrapServer,
        ExecutionUnit::BoardDaemon => ApplicationUnit::BoardDaemon,
        ExecutionUnit::VideoProxy => ApplicationUnit::VideoProxy,
        ExecutionUnit::Owner => ApplicationUnit::Owner,
    }
}

pub(crate) const fn start_unit(unit: ExecutionStartUnit) -> NativeStartUnit {
    match unit {
        ExecutionStartUnit::Owner => NativeStartUnit::Owner,
        ExecutionStartUnit::VideoProxy => NativeStartUnit::VideoProxy,
        ExecutionStartUnit::BoardDaemon => NativeStartUnit::BoardDaemon,
        ExecutionStartUnit::CurrentServer => NativeStartUnit::Server(SlotServer::Current),
        ExecutionStartUnit::CandidateServer => NativeStartUnit::Server(SlotServer::Candidate),
    }
}

/// Signed payload path of the executable a start unit runs.
pub(crate) const fn executable_path(unit: NativeStartUnit) -> &'static str {
    match unit {
        NativeStartUnit::Owner => FILES[0],
        NativeStartUnit::VideoProxy => FILES[1],
        NativeStartUnit::BoardDaemon => FILES[2],
        NativeStartUnit::Server(_) => FILES[3],
    }
}

fn unit_index(unit: ApplicationUnit) -> usize {
    MANAGED_UNITS
        .iter()
        .position(|managed| *managed == unit)
        .expect("every application unit is managed")
}

fn refusal(error: &ApplicationStageError) -> ExecutionErrorCode {
    match error {
        ApplicationStageError::StagerBusy => ExecutionErrorCode::Busy,
        ApplicationStageError::Io(_) => ExecutionErrorCode::Unavailable,
        _ => ExecutionErrorCode::Refused,
    }
}

fn composition_error(error: &CompositionError) -> ExecutionErrorCode {
    match error {
        CompositionError::Deadline => ExecutionErrorCode::Deadline,
        CompositionError::IncompatibleStartup(_)
        | CompositionError::IncompletePayload
        | CompositionError::SelectionMismatch => ExecutionErrorCode::Refused,
        _ => ExecutionErrorCode::Unavailable,
    }
}

pub struct ExecutionSession<'store, B> {
    store: &'store mut NativeExecutionStore,
    ledger: &'store SelectionLedgerStore,
    updater_root: &'store Path,
    backend: B,
    deadline: Instant,
    requests: u16,
    stopped: [bool; MANAGED_UNITS.len()],
    selected: Option<VerifiedSlotFiles>,
    finished: bool,
}

impl<'store, B: ExecutionBackend> ExecutionSession<'store, B> {
    /// Reconcile queued jobs before any request is accepted. A failed
    /// reconciliation leaves nothing dispatched and reports `Busy`.
    pub fn begin(
        store: &'store mut NativeExecutionStore,
        ledger: &'store SelectionLedgerStore,
        updater_root: &'store Path,
        mut backend: B,
        deadline: Instant,
    ) -> Result<Self, ExecutionErrorCode> {
        let now = Instant::now();
        if deadline <= now || deadline > now + MAX_EXECUTION_SESSION {
            return Err(ExecutionErrorCode::Deadline);
        }
        backend.reconcile().map_err(|_| ExecutionErrorCode::Busy)?;
        Ok(Self {
            store,
            ledger,
            updater_root,
            backend,
            deadline,
            requests: 0,
            stopped: [false; MANAGED_UNITS.len()],
            selected: None,
            finished: false,
        })
    }

    pub fn finished(&self) -> bool {
        self.finished
    }

    pub fn handle(&mut self, request: ExecutionRequest) -> ExecutionResponse {
        let error = |code| ExecutionResponse::Error { code };
        if self.finished {
            return error(ExecutionErrorCode::Deadline);
        }
        self.requests = self.requests.saturating_add(1);
        if Instant::now() >= self.deadline || self.requests > MAX_EXECUTION_SESSION_REQUESTS {
            self.finished = true;
            return error(ExecutionErrorCode::Deadline);
        }
        match request {
            ExecutionRequest::Prepare { slot_id } => {
                match self.store.prepare_from_slot(self.updater_root, &slot_id) {
                    Ok(prepared) => ExecutionResponse::Prepared {
                        slot_id: prepared.inventory().slot().slot_id.clone(),
                    },
                    Err(failure) => error(refusal(&failure)),
                }
            }
            ExecutionRequest::Select { slot_id } => {
                if !self.stopped.iter().all(|stopped| *stopped) {
                    return error(ExecutionErrorCode::Refused);
                }
                self.selected = None;
                match self.store.select(self.ledger, &slot_id) {
                    Ok(selected) => {
                        self.selected = Some(selected.into_inventory());
                        ExecutionResponse::Done {}
                    }
                    Err(failure) => error(refusal(&failure)),
                }
            }
            ExecutionRequest::Stop { unit } => {
                let unit = application_unit(unit);
                self.stopped[unit_index(unit)] = false;
                match self.backend.stop(unit) {
                    Ok(()) => ExecutionResponse::Done {},
                    Err(failure) => error(composition_error(&failure)),
                }
            }
            ExecutionRequest::ObserveStop { unit } => {
                let unit = application_unit(unit);
                match self.backend.observe_stop(unit) {
                    Ok(observation) => {
                        self.stopped[unit_index(unit)] = observation == StopObservation::Clean;
                        ExecutionResponse::Stop {
                            observation: match observation {
                                StopObservation::Clean => ExecutionStopObservation::Clean,
                                StopObservation::RunningOrQueued => {
                                    ExecutionStopObservation::RunningOrQueued
                                }
                                StopObservation::Failed => ExecutionStopObservation::Failed,
                                StopObservation::Unknown => ExecutionStopObservation::Unknown,
                            },
                        }
                    }
                    Err(failure) => error(composition_error(&failure)),
                }
            }
            ExecutionRequest::ValidateStart { unit } => {
                match self.backend.validate_start(start_unit(unit)) {
                    Ok(()) => ExecutionResponse::Done {},
                    Err(failure) => error(composition_error(&failure)),
                }
            }
            ExecutionRequest::Start { unit } => {
                // Only a slot selected in this session, after quiescence, runs.
                if self.selected.is_none() {
                    return error(ExecutionErrorCode::Refused);
                }
                let unit = start_unit(unit);
                self.stopped[unit_index(unit.unit())] = false;
                match self.backend.start(unit) {
                    Ok(()) => ExecutionResponse::Done {},
                    Err(failure) => error(composition_error(&failure)),
                }
            }
            ExecutionRequest::ObserveIdentity { unit } => {
                let Some(selected) = &self.selected else {
                    return error(ExecutionErrorCode::Refused);
                };
                let unit = start_unit(unit);
                let path = executable_path(unit);
                let (Some(file), Some(signed)) = (
                    selected.files().get(path),
                    selected
                        .slot()
                        .manifest
                        .files
                        .iter()
                        .find(|file| file.path == path),
                ) else {
                    return error(ExecutionErrorCode::Refused);
                };
                match self.backend.observe_identity(unit.unit(), file) {
                    Ok(identity) => ExecutionResponse::Identity {
                        identity: ExecutionIdentity {
                            pid: identity.pid,
                            start_ticks: identity.start_ticks,
                            executable_device: identity.executable_device,
                            executable_inode: identity.executable_inode,
                            slot_id: selected.slot().slot_id.clone(),
                            executable_sha256: signed.sha256.clone(),
                        },
                    },
                    Err(failure) => error(composition_error(&failure)),
                }
            }
            ExecutionRequest::Probe { probe } => {
                let Some(selected) = &self.selected else {
                    return error(ExecutionErrorCode::Refused);
                };
                match self.backend.probe(probe, selected) {
                    Ok(()) => ExecutionResponse::Ready {},
                    Err(failure) => error(composition_error(&failure)),
                }
            }
        }
    }
}

/// UID of the connected peer from SO_PEERCRED.
pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: stream owns the live socket; the output pointers refer to fully
    // initialized objects of the exact Linux SO_PEERCRED ABI sizes.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of::<libc::ucred>() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(credentials.uid)
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| io::ErrorKind::TimedOut.into())
}

/// Read one frame; `Ok(None)` is a clean EOF at a frame boundary.
fn read_frame(stream: &mut UnixStream, deadline: Instant) -> io::Result<Option<Vec<u8>>> {
    stream.set_read_timeout(Some(remaining(deadline)?))?;
    let mut prefix = [0_u8; 4];
    let mut filled = 0;
    while filled < prefix.len() {
        match stream.read(&mut prefix[filled..])? {
            0 if filled == 0 => return Ok(None),
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            count => filled += count,
        }
    }
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_EXECUTION_MESSAGE_BYTES {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut body = vec![0_u8; length];
    stream.set_read_timeout(Some(remaining(deadline)?))?;
    stream.read_exact(&mut body)?;
    Ok(Some(body))
}

fn write_frame(stream: &mut UnixStream, bytes: &[u8], deadline: Instant) -> io::Result<()> {
    stream.set_write_timeout(Some(remaining(deadline)?))?;
    let mut frame = Vec::with_capacity(4 + bytes.len());
    frame.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    frame.extend_from_slice(bytes);
    stream.write_all(&frame)
}

fn respond(
    stream: &mut UnixStream,
    request_id: u64,
    response: ExecutionResponse,
    deadline: Instant,
) -> io::Result<()> {
    let bytes = encode_response(&ExecutionResponseEnvelope {
        schema_version: bli_kvm_update_protocol::UPDATE_SCHEMA_VERSION,
        request_id,
        response,
    })
    .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    write_frame(stream, &bytes, deadline)
}

/// Serve one connection. The peer UID is checked before reading; malformed
/// frames and non-increasing request IDs end the session without a response.
/// Returning ends dispatch but does not cancel jobs systemd already accepted;
/// the next session reconciles them in `ExecutionSession::begin`.
pub fn serve_connection<'store, B: ExecutionBackend>(
    mut stream: UnixStream,
    expected_uid: u32,
    session: impl FnOnce(Instant) -> Result<ExecutionSession<'store, B>, ExecutionErrorCode>,
) -> io::Result<()> {
    let deadline = Instant::now() + MAX_EXECUTION_SESSION;
    if peer_uid(&stream)? != expected_uid {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    let mut last_request = 0_u64;
    let first = match read_frame(&mut stream, deadline)? {
        Some(frame) => frame,
        None => return Ok(()),
    };
    let first = decode_request(&first).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    let mut session = match session(deadline) {
        Ok(session) => session,
        Err(code) => {
            return respond(
                &mut stream,
                first.request_id,
                ExecutionResponse::Error { code },
                deadline,
            );
        }
    };
    let mut next: Option<ExecutionRequestEnvelope> = Some(first);
    loop {
        let envelope = match next.take() {
            Some(envelope) => envelope,
            None => match read_frame(&mut stream, deadline)? {
                Some(frame) => decode_request(&frame)
                    .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?,
                None => return Ok(()),
            },
        };
        if envelope.request_id <= last_request {
            return Err(io::ErrorKind::InvalidData.into());
        }
        last_request = envelope.request_id;
        let response = session.handle(envelope.request);
        respond(&mut stream, envelope.request_id, response, deadline)?;
        if session.finished() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, os::unix::fs::MetadataExt, rc::Rc};

    use bli_kvm_update_protocol::execution::{decode_response, encode_request};
    use bli_kvm_update_stager::{ApplicationSlotStager, StageQuota};

    use super::*;
    use crate::native_execution::{ExecutionPolicy, NativeExecutionStore};

    #[derive(Clone, Default)]
    struct Fake {
        calls: Rc<RefCell<Vec<String>>>,
        refuse_reconcile: bool,
    }

    impl ExecutionBackend for Fake {
        fn reconcile(&mut self) -> Result<(), CompositionError> {
            self.calls.borrow_mut().push("reconcile".into());
            if self.refuse_reconcile {
                Err(CompositionError::Controller)
            } else {
                Ok(())
            }
        }
        fn stop(&mut self, unit: ApplicationUnit) -> Result<(), CompositionError> {
            self.calls
                .borrow_mut()
                .push(format!("stop {}", unit.name()));
            Ok(())
        }
        fn observe_stop(
            &mut self,
            _: ApplicationUnit,
        ) -> Result<StopObservation, CompositionError> {
            Ok(StopObservation::Clean)
        }
        fn validate_start(&self, _: NativeStartUnit) -> Result<(), CompositionError> {
            Ok(())
        }
        fn start(&mut self, unit: NativeStartUnit) -> Result<(), CompositionError> {
            self.calls
                .borrow_mut()
                .push(format!("start {}", unit.unit().name()));
            Ok(())
        }
        fn observe_identity(
            &self,
            _: ApplicationUnit,
            expected: &File,
        ) -> Result<ProcessIdentity, CompositionError> {
            let metadata = expected.metadata().unwrap();
            Ok(ProcessIdentity {
                pid: 42,
                start_ticks: 7,
                executable_device: metadata.dev(),
                executable_inode: metadata.ino(),
            })
        }
        fn probe(
            &mut self,
            probe: ExecutionProbe,
            _: &VerifiedSlotFiles,
        ) -> Result<(), CompositionError> {
            self.calls.borrow_mut().push(format!("probe {probe:?}"));
            Ok(())
        }
    }

    struct Fixture {
        updater: tempfile::TempDir,
        _protected: tempfile::TempDir,
        _ledger_root: tempfile::TempDir,
        store: NativeExecutionStore,
        ledger: SelectionLedgerStore,
        slot_id: String,
    }

    fn fixture() -> Fixture {
        let updater = tempfile::tempdir().unwrap();
        let (trust, _, archive) = crate::slot_archive::fixture::signed_archive(15, 1);
        let slot_id = ApplicationSlotStager::open(updater.path())
            .unwrap()
            .stage_reader(archive.as_slice(), archive.len() as u64, &trust)
            .unwrap()
            .slot_id;
        let protected = tempfile::tempdir().unwrap();
        let ledger_root = tempfile::tempdir().unwrap();
        let store = NativeExecutionStore::fixture(
            protected.path(),
            trust,
            ExecutionPolicy {
                system_sequence: 1,
                minimum_application_sequence: 14,
                quota: StageQuota {
                    max_slots: 4,
                    max_payload_bytes: 1024 * 1024,
                },
            },
        );
        let ledger = SelectionLedgerStore::fixture(ledger_root.path());
        Fixture {
            updater,
            _protected: protected,
            _ledger_root: ledger_root,
            store,
            ledger,
            slot_id,
        }
    }

    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(30)
    }

    fn quiesce(session: &mut ExecutionSession<'_, Fake>) {
        for unit in [
            ExecutionUnit::BoardServer,
            ExecutionUnit::CurrentServer,
            ExecutionUnit::CandidateServer,
            ExecutionUnit::BootstrapServer,
            ExecutionUnit::BoardDaemon,
            ExecutionUnit::VideoProxy,
            ExecutionUnit::Owner,
        ] {
            assert_eq!(
                session.handle(ExecutionRequest::Stop { unit }),
                ExecutionResponse::Done {}
            );
            assert_eq!(
                session.handle(ExecutionRequest::ObserveStop { unit }),
                ExecutionResponse::Stop {
                    observation: ExecutionStopObservation::Clean
                }
            );
        }
    }

    const REFUSED: ExecutionResponse = ExecutionResponse::Error {
        code: ExecutionErrorCode::Refused,
    };

    #[test]
    fn selection_requires_quiescence_and_start_requires_selection() {
        let mut fixture = fixture();
        let fake = Fake::default();
        let slot_id = fixture.slot_id.clone();
        let mut session = ExecutionSession::begin(
            &mut fixture.store,
            &fixture.ledger,
            fixture.updater.path(),
            fake.clone(),
            deadline(),
        )
        .unwrap();
        let owner = ExecutionStartUnit::Owner;
        assert_eq!(
            session.handle(ExecutionRequest::Start { unit: owner }),
            REFUSED
        );
        assert_eq!(
            session.handle(ExecutionRequest::ObserveIdentity { unit: owner }),
            REFUSED
        );
        assert_eq!(
            session.handle(ExecutionRequest::Probe {
                probe: ExecutionProbe::Web
            }),
            REFUSED
        );
        assert_eq!(
            session.handle(ExecutionRequest::Prepare {
                slot_id: slot_id.clone()
            }),
            ExecutionResponse::Prepared {
                slot_id: slot_id.clone()
            }
        );
        assert_eq!(
            session.handle(ExecutionRequest::Select {
                slot_id: slot_id.clone()
            }),
            REFUSED
        );
        quiesce(&mut session);
        assert_eq!(
            session.handle(ExecutionRequest::Select {
                slot_id: slot_id.clone()
            }),
            ExecutionResponse::Done {}
        );
        assert_eq!(
            session.handle(ExecutionRequest::Start { unit: owner }),
            ExecutionResponse::Done {}
        );
        // A started unit is no longer quiesced: a new selection needs a new stop.
        assert_eq!(
            session.handle(ExecutionRequest::Select {
                slot_id: slot_id.clone()
            }),
            REFUSED
        );
        let ExecutionResponse::Identity { identity } =
            session.handle(ExecutionRequest::ObserveIdentity { unit: owner })
        else {
            panic!("identity expected");
        };
        assert_eq!(identity.slot_id, slot_id);
        let signed = crate::slot_archive::fixture::signed_archive(15, 1).1;
        let expected = signed
            .files
            .iter()
            .find(|file| file.path == "libexec/bli-kvm-control-broker")
            .unwrap();
        assert_eq!(identity.executable_sha256, expected.sha256);
        let protected = fixture_protected_inode(&slot_id, &session);
        assert_eq!(
            (identity.executable_device, identity.executable_inode),
            protected
        );
        assert_eq!(
            session.handle(ExecutionRequest::Probe {
                probe: ExecutionProbe::Web
            }),
            ExecutionResponse::Ready {}
        );
        assert_eq!(
            fake.calls.borrow().first().map(String::as_str),
            Some("reconcile")
        );
        assert!(
            fake.calls
                .borrow()
                .contains(&"start hardened-blikvm-board-owner.service".to_string())
        );
    }

    fn fixture_protected_inode(slot_id: &str, session: &ExecutionSession<'_, Fake>) -> (u64, u64) {
        let selected = session.selected.as_ref().unwrap();
        assert_eq!(selected.slot().slot_id, slot_id);
        let metadata = selected.files()["libexec/bli-kvm-control-broker"]
            .metadata()
            .unwrap();
        (metadata.dev(), metadata.ino())
    }

    #[test]
    fn failed_reconciliation_budget_and_deadline_end_the_session() {
        let mut fixture = fixture();
        let refusing = Fake {
            refuse_reconcile: true,
            ..Fake::default()
        };
        assert_eq!(
            ExecutionSession::begin(
                &mut fixture.store,
                &fixture.ledger,
                fixture.updater.path(),
                refusing.clone(),
                deadline(),
            )
            .err(),
            Some(ExecutionErrorCode::Busy)
        );
        assert_eq!(*refusing.calls.borrow(), ["reconcile"]);
        for invalid in [Instant::now(), Instant::now() + MAX_EXECUTION_SESSION * 2] {
            assert_eq!(
                ExecutionSession::begin(
                    &mut fixture.store,
                    &fixture.ledger,
                    fixture.updater.path(),
                    Fake::default(),
                    invalid,
                )
                .err(),
                Some(ExecutionErrorCode::Deadline)
            );
        }

        let mut session = ExecutionSession::begin(
            &mut fixture.store,
            &fixture.ledger,
            fixture.updater.path(),
            Fake::default(),
            deadline(),
        )
        .unwrap();
        let observe = || ExecutionRequest::ObserveStop {
            unit: ExecutionUnit::Owner,
        };
        for _ in 0..MAX_EXECUTION_SESSION_REQUESTS {
            assert!(matches!(
                session.handle(observe()),
                ExecutionResponse::Stop { .. }
            ));
        }
        let exhausted = ExecutionResponse::Error {
            code: ExecutionErrorCode::Deadline,
        };
        assert_eq!(session.handle(observe()), exhausted);
        assert!(session.finished());
        assert_eq!(session.handle(observe()), exhausted);
    }

    fn frame(request_id: u64, request: ExecutionRequest) -> Vec<u8> {
        let bytes = encode_request(&ExecutionRequestEnvelope {
            schema_version: 1,
            request_id,
            request,
        })
        .unwrap();
        let mut frame = (bytes.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&bytes);
        frame
    }

    fn read_response(stream: &mut UnixStream) -> ExecutionResponseEnvelope {
        let mut prefix = [0_u8; 4];
        stream.read_exact(&mut prefix).unwrap();
        let mut body = vec![0_u8; u32::from_be_bytes(prefix) as usize];
        stream.read_exact(&mut body).unwrap();
        decode_response(&body).unwrap()
    }

    /// Closing with request bytes unread may reset the stream; either way
    /// no response is sent.
    fn closed(read: io::Result<usize>) -> bool {
        match read {
            Ok(count) => count == 0,
            Err(error) => error.kind() == io::ErrorKind::ConnectionReset,
        }
    }

    fn own_uid() -> u32 {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() }
    }

    fn serve(fixture: &mut Fixture, server: UnixStream, uid: u32) -> io::Result<()> {
        let store = &mut fixture.store;
        let ledger = &fixture.ledger;
        let updater = fixture.updater.path();
        serve_connection(server, uid, |deadline| {
            ExecutionSession::begin(store, ledger, updater, Fake::default(), deadline)
        })
    }

    #[test]
    fn transport_authenticates_the_peer_and_ends_on_protocol_violations() {
        let mut fixture = fixture();
        let owner = || ExecutionRequest::ObserveStop {
            unit: ExecutionUnit::Owner,
        };

        let (mut client, server) = UnixStream::pair().unwrap();
        client.write_all(&frame(1, owner())).unwrap();
        let error = serve(&mut fixture, server, own_uid().wrapping_add(1)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut byte = [0_u8; 1];
        assert!(closed(client.read(&mut byte)));

        let (mut client, server) = UnixStream::pair().unwrap();
        client.write_all(&frame(1, owner())).unwrap();
        client.write_all(&frame(2, owner())).unwrap();
        client.write_all(&frame(2, owner())).unwrap();
        let error = serve(&mut fixture, server, own_uid()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(read_response(&mut client).request_id, 1);
        assert_eq!(read_response(&mut client).request_id, 2);
        assert!(closed(client.read(&mut byte)));

        let (mut client, server) = UnixStream::pair().unwrap();
        client
            .write_all(&((MAX_EXECUTION_MESSAGE_BYTES + 1) as u32).to_be_bytes())
            .unwrap();
        assert_eq!(
            serve(&mut fixture, server, own_uid()).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );

        let (mut client, server) = UnixStream::pair().unwrap();
        client.write_all(&frame(5, owner())).unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        serve(&mut fixture, server, own_uid()).unwrap();
        assert_eq!(
            read_response(&mut client).response,
            ExecutionResponse::Stop {
                observation: ExecutionStopObservation::Clean
            }
        );
    }
}
