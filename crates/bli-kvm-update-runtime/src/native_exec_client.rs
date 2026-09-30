//! Coordinator side of the execution helper session.
//! Implements the existing native controller traits, so the unchanged native
//! flows drive the root helper instead of systemctl. One client is one session:
//! it must be created for one composition attempt and dropped afterwards.

use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs::File,
    io::{self, Read, Write},
    os::unix::{fs::FileExt, fs::MetadataExt, net::UnixStream},
    path::Path,
    rc::Rc,
    time::Instant,
};

use bli_kvm_update_protocol::execution::{
    ExecutionErrorCode, ExecutionProbe, ExecutionRequest, ExecutionRequestEnvelope,
    ExecutionResponse, ExecutionStartUnit, ExecutionStopObservation, ExecutionUnit,
    MAX_EXECUTION_MESSAGE_BYTES, decode_response, encode_request,
};
use sha2::{Digest, Sha256};

use crate::{
    composition::{ApplicationUnit, CompositionError, ServiceController, StopObservation},
    native_exec_server::MAX_EXECUTION_SESSION,
    native_identity::{
        NativeIdentityController, NativeStartController, NativeStartUnit, SlotServer,
    },
    process_identity::ProcessIdentity,
};

/// Fixed helper socket; the directory is root-owned and not writable by the
/// coordinator. Never taken from configuration supplied by the coordinator.
pub const EXECUTION_SOCKET: &str = "/run/hardened-blikvm-exec-api/exec.sock";

struct Session {
    stream: UnixStream,
    deadline: Instant,
    next_request: u64,
    selected: Option<String>,
    digests: BTreeMap<(u64, u64), String>,
    closed: bool,
}

/// Cloning shares the same session, so readiness probes can run while the
/// controller is borrowed by a native flow.
#[derive(Clone)]
pub struct ExecutionClient(Rc<RefCell<Session>>);

const fn execution_unit(unit: ApplicationUnit) -> ExecutionUnit {
    match unit {
        ApplicationUnit::BoardServer => ExecutionUnit::BoardServer,
        ApplicationUnit::CurrentServer => ExecutionUnit::CurrentServer,
        ApplicationUnit::CandidateServer => ExecutionUnit::CandidateServer,
        ApplicationUnit::BootstrapServer => ExecutionUnit::BootstrapServer,
        ApplicationUnit::BoardDaemon => ExecutionUnit::BoardDaemon,
        ApplicationUnit::VideoProxy => ExecutionUnit::VideoProxy,
        ApplicationUnit::Owner => ExecutionUnit::Owner,
    }
}

const fn execution_start_unit(unit: NativeStartUnit) -> ExecutionStartUnit {
    match unit {
        NativeStartUnit::Owner => ExecutionStartUnit::Owner,
        NativeStartUnit::VideoProxy => ExecutionStartUnit::VideoProxy,
        NativeStartUnit::BoardDaemon => ExecutionStartUnit::BoardDaemon,
        NativeStartUnit::Server(SlotServer::Current) => ExecutionStartUnit::CurrentServer,
        NativeStartUnit::Server(SlotServer::Candidate) => ExecutionStartUnit::CandidateServer,
    }
}

fn startable(unit: ApplicationUnit) -> Option<ExecutionStartUnit> {
    Some(match unit {
        ApplicationUnit::Owner => ExecutionStartUnit::Owner,
        ApplicationUnit::VideoProxy => ExecutionStartUnit::VideoProxy,
        ApplicationUnit::BoardDaemon => ExecutionStartUnit::BoardDaemon,
        ApplicationUnit::CurrentServer => ExecutionStartUnit::CurrentServer,
        ApplicationUnit::CandidateServer => ExecutionStartUnit::CandidateServer,
        _ => return None,
    })
}

fn failure(code: ExecutionErrorCode, refused: CompositionError) -> CompositionError {
    match code {
        ExecutionErrorCode::Refused => refused,
        ExecutionErrorCode::Deadline => CompositionError::Deadline,
        ExecutionErrorCode::Busy | ExecutionErrorCode::Unavailable => CompositionError::Controller,
    }
}

fn digest(file: &File) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut offset = 0_u64;
    loop {
        let count = file.read_at(&mut buffer, offset)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        offset += count as u64;
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

impl ExecutionClient {
    /// Connect to the helper socket for one attempt ending at `deadline`.
    pub fn connect(path: &Path, deadline: Instant) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        Self::from_stream(stream, deadline)
    }

    pub(crate) fn from_stream(stream: UnixStream, deadline: Instant) -> io::Result<Self> {
        let now = Instant::now();
        if deadline <= now || deadline > now + MAX_EXECUTION_SESSION {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(Self(Rc::new(RefCell::new(Session {
            stream,
            deadline,
            next_request: 1,
            selected: None,
            digests: BTreeMap::new(),
            closed: false,
        }))))
    }

    fn exchange(&self, request: ExecutionRequest) -> Result<ExecutionResponse, CompositionError> {
        let mut session = self.0.borrow_mut();
        if session.closed {
            return Err(CompositionError::Controller);
        }
        let remaining = session
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(CompositionError::Deadline)?;
        let request_id = session.next_request;
        session.next_request += 1;
        let result = (|| -> io::Result<ExecutionResponse> {
            let bytes = encode_request(&ExecutionRequestEnvelope {
                schema_version: bli_kvm_update_protocol::UPDATE_SCHEMA_VERSION,
                request_id,
                request,
            })
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
            session.stream.set_write_timeout(Some(remaining))?;
            session.stream.set_read_timeout(Some(remaining))?;
            let mut frame = (bytes.len() as u32).to_be_bytes().to_vec();
            frame.extend_from_slice(&bytes);
            session.stream.write_all(&frame)?;
            let mut prefix = [0_u8; 4];
            session.stream.read_exact(&mut prefix)?;
            let length = u32::from_be_bytes(prefix) as usize;
            if length == 0 || length > MAX_EXECUTION_MESSAGE_BYTES {
                return Err(io::ErrorKind::InvalidData.into());
            }
            let mut body = vec![0_u8; length];
            session.stream.read_exact(&mut body)?;
            let envelope =
                decode_response(&body).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
            if envelope.request_id != request_id {
                return Err(io::ErrorKind::InvalidData.into());
            }
            Ok(envelope.response)
        })();
        match result {
            Ok(response) => Ok(response),
            Err(error) => {
                // Never replay: an unanswered request may already have acted.
                session.closed = true;
                Err(
                    if error.kind() == io::ErrorKind::TimedOut
                        || error.kind() == io::ErrorKind::WouldBlock
                    {
                        CompositionError::Deadline
                    } else {
                        CompositionError::Controller
                    },
                )
            }
        }
    }

    fn done(
        &self,
        request: ExecutionRequest,
        refused: CompositionError,
    ) -> Result<(), CompositionError> {
        match self.exchange(request)? {
            ExecutionResponse::Done {} => Ok(()),
            ExecutionResponse::Error { code } => Err(failure(code, refused)),
            _ => Err(CompositionError::Controller),
        }
    }

    /// Readiness probe for one started unit, run by the helper. Server units
    /// map to the signed web tree probe.
    pub fn probe(&self, unit: ApplicationUnit) -> Result<(), CompositionError> {
        let probe = match unit {
            ApplicationUnit::Owner => ExecutionProbe::Owner,
            ApplicationUnit::VideoProxy => ExecutionProbe::VideoProxy,
            ApplicationUnit::BoardDaemon => ExecutionProbe::BoardDaemon,
            ApplicationUnit::CurrentServer | ApplicationUnit::CandidateServer => {
                ExecutionProbe::Web
            }
            _ => return Err(CompositionError::Controller),
        };
        match self.exchange(ExecutionRequest::Probe { probe })? {
            ExecutionResponse::Ready {} => Ok(()),
            ExecutionResponse::Error { code } => Err(failure(code, CompositionError::Controller)),
            _ => Err(CompositionError::Controller),
        }
    }
}

impl ServiceController for ExecutionClient {
    fn stop(&mut self, unit: ApplicationUnit) -> Result<(), CompositionError> {
        self.done(
            ExecutionRequest::Stop {
                unit: execution_unit(unit),
            },
            CompositionError::Controller,
        )
    }

    fn observe_stop(&mut self, unit: ApplicationUnit) -> Result<StopObservation, CompositionError> {
        match self.exchange(ExecutionRequest::ObserveStop {
            unit: execution_unit(unit),
        })? {
            ExecutionResponse::Stop { observation } => Ok(match observation {
                ExecutionStopObservation::Clean => StopObservation::Clean,
                ExecutionStopObservation::RunningOrQueued => StopObservation::RunningOrQueued,
                ExecutionStopObservation::StoppedAfterFailure => {
                    StopObservation::StoppedAfterFailure
                }
                ExecutionStopObservation::Failed => StopObservation::Failed,
                ExecutionStopObservation::Unknown => StopObservation::Unknown,
            }),
            ExecutionResponse::Error { code } => Err(failure(code, CompositionError::Controller)),
            _ => Err(CompositionError::Controller),
        }
    }
}

impl NativeIdentityController for ExecutionClient {
    /// The helper binds the process to its protected copy. The coordinator
    /// requires the reported slot to be the one it selected and the signed
    /// digest to equal the bytes of its own verified slot file.
    fn observe_identity(
        &self,
        unit: ApplicationUnit,
        expected: &File,
    ) -> Result<ProcessIdentity, CompositionError> {
        let start = startable(unit).ok_or(CompositionError::Controller)?;
        let ExecutionResponse::Identity { identity } =
            self.exchange(ExecutionRequest::ObserveIdentity { unit: start })?
        else {
            return Err(CompositionError::Controller);
        };
        let metadata = expected
            .metadata()
            .map_err(|_| CompositionError::Controller)?;
        let key = (metadata.dev(), metadata.ino());
        let mut session = self.0.borrow_mut();
        if session.selected.as_deref() != Some(identity.slot_id.as_str()) {
            return Err(CompositionError::SelectionMismatch);
        }
        let expected_digest = match session.digests.get(&key) {
            Some(value) => value.clone(),
            None => {
                let value = digest(expected).map_err(|_| CompositionError::Controller)?;
                session.digests.insert(key, value.clone());
                value
            }
        };
        if expected_digest != identity.executable_sha256 {
            return Err(CompositionError::SelectionMismatch);
        }
        Ok(ProcessIdentity {
            pid: identity.pid,
            start_ticks: identity.start_ticks,
            executable_device: identity.executable_device,
            executable_inode: identity.executable_inode,
        })
    }
}

impl NativeStartController for ExecutionClient {
    fn validate_start(&self, unit: NativeStartUnit) -> Result<(), CompositionError> {
        self.done(
            ExecutionRequest::ValidateStart {
                unit: execution_start_unit(unit),
            },
            CompositionError::IncompatibleStartup(unit.unit()),
        )
    }

    fn start(&mut self, unit: NativeStartUnit) -> Result<(), CompositionError> {
        self.done(
            ExecutionRequest::Start {
                unit: execution_start_unit(unit),
            },
            CompositionError::IncompatibleStartup(unit.unit()),
        )
    }

    fn prepare(&mut self, slot_id: &str) -> Result<(), CompositionError> {
        match self.exchange(ExecutionRequest::Prepare {
            slot_id: slot_id.to_string(),
        })? {
            ExecutionResponse::Prepared { slot_id: prepared } if prepared == slot_id => Ok(()),
            ExecutionResponse::Error { code } => {
                Err(failure(code, CompositionError::IncompletePayload))
            }
            _ => Err(CompositionError::Controller),
        }
    }

    fn select(&mut self, slot_id: &str) -> Result<(), CompositionError> {
        self.0.borrow_mut().selected = None;
        self.done(
            ExecutionRequest::Select {
                slot_id: slot_id.to_string(),
            },
            CompositionError::SelectionMismatch,
        )?;
        self.0.borrow_mut().selected = Some(slot_id.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{os::unix::fs::MetadataExt, time::Duration};

    use super::*;
    use crate::native_exec_server::{
        ExecutionSession, serve_connection,
        testing::{Fake, fixture},
    };

    const UNITS: [ApplicationUnit; 7] = [
        ApplicationUnit::BoardServer,
        ApplicationUnit::CurrentServer,
        ApplicationUnit::CandidateServer,
        ApplicationUnit::BootstrapServer,
        ApplicationUnit::BoardDaemon,
        ApplicationUnit::VideoProxy,
        ApplicationUnit::Owner,
    ];

    #[test]
    fn client_drives_a_helper_session_and_binds_identity_to_signed_bytes() {
        let mut fixture = fixture();
        let slot_id = fixture.slot_id.clone();
        let updater_slot = fixture.updater.path().join(&slot_id);
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        std::thread::scope(|scope| {
            let server = scope.spawn(|| {
                let store = &mut fixture.store;
                let ledger = &fixture.ledger;
                let updater = fixture.updater.path();
                serve_connection(server_stream, uid, |deadline| {
                    ExecutionSession::begin(store, ledger, updater, Fake::default(), deadline)
                })
            });
            let mut client = ExecutionClient::from_stream(
                client_stream,
                Instant::now() + Duration::from_secs(30),
            )
            .unwrap();
            client.prepare(&slot_id).unwrap();
            assert!(matches!(
                client.select(&slot_id),
                Err(CompositionError::SelectionMismatch)
            ));
            for unit in UNITS {
                assert_eq!(client.observe_stop(unit).unwrap(), StopObservation::Clean);
            }
            client.select(&slot_id).unwrap();
            client.validate_start(NativeStartUnit::Owner).unwrap();
            client.start(NativeStartUnit::Owner).unwrap();

            let signed = File::open(updater_slot.join("libexec/bli-kvm-control-broker")).unwrap();
            let identity = client
                .observe_identity(ApplicationUnit::Owner, &signed)
                .unwrap();
            let source = signed.metadata().unwrap();
            // The process identity is the protected copy, not the updater file.
            assert_ne!(
                (identity.executable_device, identity.executable_inode),
                (source.dev(), source.ino())
            );
            assert_eq!(identity.pid, 42);

            // A different signed role does not match the unit's digest.
            let other = File::open(updater_slot.join("libexec/bli-kvm-video-proxy")).unwrap();
            assert!(matches!(
                client.observe_identity(ApplicationUnit::Owner, &other),
                Err(CompositionError::SelectionMismatch)
            ));
            assert!(matches!(
                client.observe_identity(ApplicationUnit::BootstrapServer, &signed),
                Err(CompositionError::Controller)
            ));
            let probes = client.clone();
            probes.probe(ApplicationUnit::CurrentServer).unwrap();
            drop(probes);
            drop(client);
            server.join().unwrap().unwrap();
        });
    }

    #[test]
    fn an_unanswered_request_closes_the_client_without_replay() {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        drop(server_stream);
        let mut client =
            ExecutionClient::from_stream(client_stream, Instant::now() + Duration::from_secs(5))
                .unwrap();
        assert!(client.stop(ApplicationUnit::Owner).is_err());
        assert!(matches!(
            client.observe_stop(ApplicationUnit::Owner),
            Err(CompositionError::Controller)
        ));
        let (stream, _) = UnixStream::pair().unwrap();
        assert!(ExecutionClient::from_stream(stream, Instant::now()).is_err());
    }
}
