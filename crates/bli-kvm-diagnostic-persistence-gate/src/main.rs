#![forbid(unsafe_code)]

//! Unshipped synthetic device gate for the exact diagnostic persistence stack.
//!
//! The binary accepts no argument, environment-selected path, key, floor, or
//! endpoint. It can run only under the separately staged test unit whose exact
//! systemd credential directory is already accepted by the key loader.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    process::ExitCode,
    sync::Arc,
    time::Duration,
};

use bli_kvm_diagnostic_coordinator::DiagnosticGenerationFloor;
use bli_kvm_diagnostic_export::{
    DiagnosticBundle, DiagnosticComponent, DiagnosticEvent, DiagnosticExporter, DiagnosticRecord,
    PressureLevel, ResourceKind,
};
use bli_kvm_diagnostic_export_protocol::{
    DIAGNOSTIC_EXPORT_REQUEST_BYTES, DiagnosticExportRequest, DiagnosticExportResponse,
    DiagnosticExportResponseFrame, decode_request, encode_response,
};
use bli_kvm_diagnostic_journal::DiagnosticJournalEntryState;
use bli_kvm_diagnostic_key_loader::load_systemd_diagnostic_journal_keys;
use bli_kvm_diagnostic_persistence::{DiagnosticPersistenceRuntime, DiagnosticPersistenceStep};
use bli_kvm_diagnostic_store::DiagnosticJournalStore;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{Mutex, Notify, oneshot},
    task::JoinSet,
    time::{sleep, timeout},
};

const EXPORT_SOCKET: &str = "/run/hardened-blikvm-diagnostics/export.sock";
const RESULT_FILE: &str = "/run/hardened-blikvm-diagnostics/gate.result";
const INSTANCE_ID: u64 = 0x424c_494b_564d_0001;
const WINDOW_ONE: u32 = 1;
const WINDOW_TWO: u32 = 2;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const STALLED_RESPONSE_DELAY: Duration = Duration::from_secs(1);
const FINAL_GENERATION: u64 = 7;
const EXPECTED_READ_REQUESTS: u32 = 7;
const EXPECTED_ACKNOWLEDGEMENT_REQUESTS: u32 = 3;
const SUCCESS_RECEIPT: &str = concat!(
    "schema=1\n",
    "result=passed\n",
    "final_generation=7\n",
    "entry_count=2\n",
    "acknowledged_entries=2\n",
    "read_requests=7\n",
    "acknowledgement_requests=3\n",
    "cancelled_acknowledgements=1\n",
    "automatic_retries=0\n",
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateError {
    Setup,
    Protocol,
    Runtime,
    Verification,
    Receipt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveWindow {
    One,
    Two,
    None,
}

struct ScriptedCollector {
    first: DiagnosticBundle,
    second: DiagnosticBundle,
    active: ActiveWindow,
    stalled_second_ack: bool,
    read_requests: u32,
    acknowledgement_requests: u32,
}

impl ScriptedCollector {
    fn new(first: DiagnosticBundle, second: DiagnosticBundle) -> Self {
        Self {
            first,
            second,
            active: ActiveWindow::One,
            stalled_second_ack: false,
            read_requests: 0,
            acknowledgement_requests: 0,
        }
    }

    fn plan(&mut self, request: DiagnosticExportRequest) -> Result<ServerAction, GateError> {
        match request {
            DiagnosticExportRequest::ReadSealed => {
                self.read_requests = self
                    .read_requests
                    .checked_add(1)
                    .ok_or(GateError::Verification)?;
                let response = match self.active {
                    ActiveWindow::One => DiagnosticExportResponse::Sealed {
                        instance_id: INSTANCE_ID,
                        window_id: WINDOW_ONE,
                        omitted_lifecycle_events: 0,
                        bundle: &self.first,
                    },
                    ActiveWindow::Two => DiagnosticExportResponse::Sealed {
                        instance_id: INSTANCE_ID,
                        window_id: WINDOW_TWO,
                        omitted_lifecycle_events: 0,
                        bundle: &self.second,
                    },
                    ActiveWindow::None => DiagnosticExportResponse::NoSealed,
                };
                encode_response(response)
                    .map(ServerAction::Respond)
                    .map_err(|_| GateError::Protocol)
            }
            DiagnosticExportRequest::Acknowledge {
                instance_id,
                window_id,
            } => {
                self.acknowledgement_requests = self
                    .acknowledgement_requests
                    .checked_add(1)
                    .ok_or(GateError::Verification)?;
                if instance_id != INSTANCE_ID
                    || !matches!(
                        (self.active, window_id),
                        (ActiveWindow::One, WINDOW_ONE) | (ActiveWindow::Two, WINDOW_TWO)
                    )
                {
                    return encode_response(DiagnosticExportResponse::AcknowledgementMismatch {
                        instance_id,
                        window_id,
                    })
                    .map(ServerAction::Respond)
                    .map_err(|_| GateError::Protocol);
                }
                match self.active {
                    ActiveWindow::One => {
                        self.active = ActiveWindow::Two;
                        encode_response(DiagnosticExportResponse::Acknowledged {
                            instance_id,
                            window_id,
                        })
                        .map(ServerAction::Respond)
                        .map_err(|_| GateError::Protocol)
                    }
                    ActiveWindow::Two if !self.stalled_second_ack => {
                        self.stalled_second_ack = true;
                        Ok(ServerAction::Stall)
                    }
                    ActiveWindow::Two => {
                        self.active = ActiveWindow::None;
                        encode_response(DiagnosticExportResponse::Acknowledged {
                            instance_id,
                            window_id,
                        })
                        .map(ServerAction::Respond)
                        .map_err(|_| GateError::Protocol)
                    }
                    ActiveWindow::None => encode_response(DiagnosticExportResponse::NoSealed)
                        .map(ServerAction::Respond)
                        .map_err(|_| GateError::Protocol),
                }
            }
        }
    }
}

// Response frames are fixed and bounded; keeping them inline avoids adding an
// allocation failure to the synthetic protocol owner.
#[allow(clippy::large_enum_variant)]
enum ServerAction {
    Respond(DiagnosticExportResponseFrame),
    Stall,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run_gate().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

async fn run_gate() -> Result<(), GateError> {
    if fs::symlink_metadata(EXPORT_SOCKET).is_ok() || fs::symlink_metadata(RESULT_FILE).is_ok() {
        return Err(GateError::Setup);
    }
    let first = diagnostic_bundle(0, DiagnosticEvent::Ready)?;
    let second = diagnostic_bundle(
        1,
        DiagnosticEvent::ResourcePressure {
            resource: ResourceKind::Temperature,
            level: PressureLevel::Elevated,
        },
    )?;
    let state = Arc::new(Mutex::new(ScriptedCollector::new(first, second)));
    let listener = UnixListener::bind(EXPORT_SOCKET).map_err(|_| GateError::Setup)?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let stall_started = Arc::new(Notify::new());
    let server_state = Arc::clone(&state);
    let server_stall = Arc::clone(&stall_started);
    let server =
        tokio::spawn(async move { serve(listener, server_state, server_stall, shutdown_rx).await });

    let keys = load_systemd_diagnostic_journal_keys().map_err(|_| GateError::Setup)?;
    let store = DiagnosticJournalStore::open().map_err(|_| GateError::Setup)?;
    let initialized = store
        .initialize(&keys.key_ring())
        .map_err(|_| GateError::Setup)?;
    if initialized.generation != 1 {
        return Err(GateError::Verification);
    }
    let mut runtime = DiagnosticPersistenceRuntime::new(keys, store);

    expect_advance(runtime.poll(floor(1)?).await, 2)?;
    expect_advance(runtime.poll(floor(2)?).await, 3)?;
    expect_advance(runtime.poll(floor(3)?).await, 4)?;
    let cancellation_floor = floor(4)?;
    timeout(REQUEST_TIMEOUT, async {
        let acknowledgement = runtime.poll(cancellation_floor);
        tokio::pin!(acknowledgement);
        tokio::select! {
            _ = stall_started.notified() => Ok(()),
            _ = &mut acknowledgement => Err(GateError::Verification),
        }
    })
    .await
    .map_err(|_| GateError::Verification)??;
    expect_advance(runtime.poll(floor(4)?).await, 5)?;
    expect_advance(runtime.poll(floor(5)?).await, 6)?;
    expect_advance(runtime.poll(floor(6)?).await, FINAL_GENERATION)?;
    match runtime.poll(floor(FINAL_GENERATION)?).await {
        Ok(DiagnosticPersistenceStep::Idle {
            generation: FINAL_GENERATION,
        }) => {}
        _ => return Err(GateError::Runtime),
    }
    drop(runtime);

    shutdown_tx.send(()).map_err(|_| GateError::Verification)?;
    server.await.map_err(|_| GateError::Verification)??;
    let counters = state.lock().await;
    if counters.active != ActiveWindow::None
        || !counters.stalled_second_ack
        || counters.read_requests != EXPECTED_READ_REQUESTS
        || counters.acknowledgement_requests != EXPECTED_ACKNOWLEDGEMENT_REQUESTS
    {
        return Err(GateError::Verification);
    }
    drop(counters);

    let verification_keys =
        load_systemd_diagnostic_journal_keys().map_err(|_| GateError::Verification)?;
    let verification_store = DiagnosticJournalStore::open().map_err(|_| GateError::Verification)?;
    let selected = verification_store
        .load_read_only(&verification_keys.key_ring(), FINAL_GENERATION)
        .map_err(|_| GateError::Verification)?;
    if selected.degraded()
        || selected.generation() != FINAL_GENERATION
        || selected.journal().entry_count() != 2
        || selected
            .journal()
            .entries()
            .any(|entry| entry.state() != DiagnosticJournalEntryState::Acknowledged)
    {
        return Err(GateError::Verification);
    }
    drop(verification_store);
    drop(verification_keys);
    write_receipt()
}

async fn serve(
    listener: UnixListener,
    state: Arc<Mutex<ScriptedCollector>>,
    stall_started: Arc<Notify>,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<(), GateError> {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.map_err(|_| GateError::Protocol)?;
                let connection_state = Arc::clone(&state);
                let connection_stall = Arc::clone(&stall_started);
                connections.spawn(async move {
                    handle_connection(stream, connection_state, connection_stall).await
                });
            }
            _ = &mut shutdown => break,
        }
    }
    connections.abort_all();
    while connections.join_next().await.is_some() {}
    Ok(())
}

async fn handle_connection(
    mut stream: UnixStream,
    state: Arc<Mutex<ScriptedCollector>>,
    stall_started: Arc<Notify>,
) -> Result<(), GateError> {
    let mut request = [0_u8; DIAGNOSTIC_EXPORT_REQUEST_BYTES];
    timeout(REQUEST_TIMEOUT, stream.read_exact(&mut request))
        .await
        .map_err(|_| GateError::Protocol)?
        .map_err(|_| GateError::Protocol)?;
    let request = decode_request(&request).map_err(|_| GateError::Protocol)?;
    let action = state.lock().await.plan(request)?;
    match action {
        ServerAction::Respond(response) => {
            timeout(REQUEST_TIMEOUT, stream.write_all(response.as_bytes()))
                .await
                .map_err(|_| GateError::Protocol)?
                .map_err(|_| GateError::Protocol)?;
            stream.shutdown().await.map_err(|_| GateError::Protocol)
        }
        ServerAction::Stall => {
            stall_started.notify_one();
            sleep(STALLED_RESPONSE_DELAY).await;
            Ok(())
        }
    }
}

fn diagnostic_bundle(
    offset_ms: u32,
    event: DiagnosticEvent,
) -> Result<DiagnosticBundle, GateError> {
    let mut exporter = DiagnosticExporter::new();
    exporter
        .push(
            DiagnosticRecord::new(offset_ms, DiagnosticComponent::PlatformBroker, event)
                .map_err(|_| GateError::Setup)?,
        )
        .map_err(|_| GateError::Setup)?;
    exporter.export().map_err(|_| GateError::Setup)
}

fn floor(generation: u64) -> Result<DiagnosticGenerationFloor, GateError> {
    DiagnosticGenerationFloor::new(generation).ok_or(GateError::Setup)
}

fn expect_advance(
    result: Result<DiagnosticPersistenceStep, impl std::error::Error>,
    expected: u64,
) -> Result<(), GateError> {
    match result {
        Ok(DiagnosticPersistenceStep::AdvanceFloor { generation }) if generation == expected => {
            Ok(())
        }
        _ => Err(GateError::Runtime),
    }
}

fn write_receipt() -> Result<(), GateError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(RESULT_FILE)
        .map_err(|_| GateError::Receipt)?;
    file.write_all(SUCCESS_RECEIPT.as_bytes())
        .map_err(|_| GateError::Receipt)?;
    file.sync_all().map_err(|_| GateError::Receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_diagnostic_export_protocol::{DecodedDiagnosticExportResponse, decode_response};

    fn response(action: ServerAction) -> DiagnosticExportResponseFrame {
        match action {
            ServerAction::Respond(frame) => frame,
            ServerAction::Stall => panic!("unexpected stall"),
        }
    }

    #[test]
    fn scripted_collector_sequence_is_exact_and_typed() {
        let first = diagnostic_bundle(0, DiagnosticEvent::Ready).expect("first");
        let second = diagnostic_bundle(1, DiagnosticEvent::Started).expect("second");
        let mut state = ScriptedCollector::new(first, second);

        let read_one = response(
            state
                .plan(DiagnosticExportRequest::ReadSealed)
                .expect("read one"),
        );
        assert!(matches!(
            decode_response(read_one.as_bytes()).expect("decode"),
            DecodedDiagnosticExportResponse::Sealed {
                instance_id: INSTANCE_ID,
                window_id: WINDOW_ONE,
                ..
            }
        ));
        let ack_one = response(
            state
                .plan(DiagnosticExportRequest::Acknowledge {
                    instance_id: INSTANCE_ID,
                    window_id: WINDOW_ONE,
                })
                .expect("ack one"),
        );
        assert!(matches!(
            decode_response(ack_one.as_bytes()).expect("decode"),
            DecodedDiagnosticExportResponse::Acknowledged {
                window_id: WINDOW_ONE,
                ..
            }
        ));
        assert!(matches!(
            state
                .plan(DiagnosticExportRequest::Acknowledge {
                    instance_id: INSTANCE_ID,
                    window_id: WINDOW_TWO,
                })
                .expect("stall"),
            ServerAction::Stall
        ));
        let ack_two = response(
            state
                .plan(DiagnosticExportRequest::Acknowledge {
                    instance_id: INSTANCE_ID,
                    window_id: WINDOW_TWO,
                })
                .expect("ack two"),
        );
        assert!(matches!(
            decode_response(ack_two.as_bytes()).expect("decode"),
            DecodedDiagnosticExportResponse::Acknowledged {
                window_id: WINDOW_TWO,
                ..
            }
        ));
        assert_eq!(state.active, ActiveWindow::None);
    }

    #[test]
    fn mismatch_preserves_the_active_window() {
        let first = diagnostic_bundle(0, DiagnosticEvent::Ready).expect("first");
        let second = diagnostic_bundle(1, DiagnosticEvent::Started).expect("second");
        let mut state = ScriptedCollector::new(first, second);
        let mismatch = response(
            state
                .plan(DiagnosticExportRequest::Acknowledge {
                    instance_id: INSTANCE_ID,
                    window_id: WINDOW_TWO,
                })
                .expect("mismatch"),
        );
        assert!(matches!(
            decode_response(mismatch.as_bytes()).expect("decode"),
            DecodedDiagnosticExportResponse::AcknowledgementMismatch { .. }
        ));
        assert_eq!(state.active, ActiveWindow::One);
    }

    #[test]
    fn receipt_and_expected_counts_are_fixed() {
        assert_eq!(EXPECTED_READ_REQUESTS, 7);
        assert_eq!(EXPECTED_ACKNOWLEDGEMENT_REQUESTS, 3);
        assert_eq!(FINAL_GENERATION, 7);
        assert_eq!(SUCCESS_RECEIPT.len(), 171);
        assert_eq!(SUCCESS_RECEIPT.lines().count(), 9);
        assert!(SUCCESS_RECEIPT.ends_with("automatic_retries=0\n"));
    }

    #[test]
    fn fixed_paths_and_bindings_are_nonempty() {
        assert_eq!(
            EXPORT_SOCKET,
            "/run/hardened-blikvm-diagnostics/export.sock"
        );
        assert_eq!(RESULT_FILE, "/run/hardened-blikvm-diagnostics/gate.result");
        assert_ne!(INSTANCE_ID, 0);
        assert_ne!(WINDOW_ONE, WINDOW_TWO);
    }
}
