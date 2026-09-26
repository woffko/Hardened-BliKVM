use crate::{
    SupervisorCliError,
    config::load_config,
    status_socket::{self, StatusSocketError},
};
use bli_kvm_update_manager::{ActivationOutcome, ApplicationUpdateManager, UpdateManagerError};
use bli_kvm_update_protocol::{
    MAX_UPDATE_MESSAGE_BYTES, UPDATE_SCHEMA_VERSION, UpdateProtocolError,
    activation::{
        self, ActivationErrorCode, ActivationReceipt, ActivationRequest, ActivationResponse,
        ActivationResponseEnvelope,
    },
    encode_frame,
};
use bli_kvm_update_runtime::{HealthChallengeStore, UpdateRuntimeError, UpdateSupervisor};
use bli_kvm_update_stager::ApplicationStageError;
use std::{
    io::{Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};

fn read_exact_before(
    stream: &mut UnixStream,
    mut bytes: &mut [u8],
    deadline: Instant,
) -> Result<(), StatusSocketError> {
    while !bytes.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|time| !time.is_zero())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "activation metadata deadline")
            })?;
        stream.set_read_timeout(Some(remaining))?;
        match stream.read(bytes) {
            Ok(0) => return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into()),
            Ok(count) => bytes = &mut bytes[count..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn receive(
    stream: &mut UnixStream,
) -> Result<activation::ActivationRequestEnvelope, StatusSocketError> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut length = [0; 4];
    read_exact_before(stream, &mut length, deadline)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_UPDATE_MESSAGE_BYTES {
        return Err(UpdateProtocolError::InvalidLength.into());
    }
    let mut bytes = vec![0; length];
    read_exact_before(stream, &mut bytes, deadline)?;
    let request = activation::decode_request(&bytes)?;
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|time| !time.is_zero())
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::TimedOut))?;
    stream.set_read_timeout(Some(remaining))?;
    if stream.read(&mut [0; 1])? != 0 {
        return Err(UpdateProtocolError::InvalidLength.into());
    }
    Ok(request)
}

pub(crate) fn serve_stdin(
    config: &Path,
    peer_user: &str,
    native: bool,
) -> Result<(), SupervisorCliError> {
    let (mut stream, _) = status_socket::authorized_stdin(peer_user)?;
    // Authenticate and fully close the bounded request before taking any
    // journal/slot locks. A slow sender cannot block watchdog or status.
    let request = receive(&mut stream)?;
    if native {
        return serve_native(config, stream, request);
    }
    let result = match request.request {
        ActivationRequest::Activate {
            slot_id,
            expected_generation,
        } => activate(config, &slot_id, expected_generation),
        // Admission requires the complete native executor. Never silently run
        // legacy activation for a client asking only for Prepared admission.
        ActivationRequest::Admit { .. } => Err(SupervisorCliError::ActivationNotDispatched),
    };
    let dispatch = result.is_ok();
    let response = ActivationResponseEnvelope {
        schema_version: UPDATE_SCHEMA_VERSION,
        request_id: request.request_id,
        response: match result {
            Ok(receipt) => ActivationResponse::AwaitingHealth { receipt },
            Err(error) => ActivationResponse::Error {
                code: error_code(&error),
            },
        },
    };
    // Manager and its locks have been dropped before response delivery and
    // before the service manager dispatches the two fixed apply services.
    let delivery = deliver_response_before(
        &mut stream,
        &response,
        Instant::now() + Duration::from_secs(2),
    );
    if dispatch {
        // Exit success authorizes the unit's fixed OnSuccess apply dispatch.
        // A lost response after AwaitingHealth must not suppress that dispatch.
        // Never return success for a rejected/no-op/replayed activation.
        let _ = delivery;
        Ok(())
    } else {
        delivery?;
        Err(SupervisorCliError::ActivationNotDispatched)
    }
}

/// Complete one bounded frame and publish EOF even when stdin/stdout retain
/// duplicate descriptors. Native admission may quiesce only after this succeeds.
fn deliver_response_before(
    stream: &mut UnixStream,
    response: &ActivationResponseEnvelope,
    deadline: Instant,
) -> Result<(), StatusSocketError> {
    let bytes = encode_frame(&activation::encode_response(response)?)?;
    let mut remaining_bytes = bytes.as_slice();
    while !remaining_bytes.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::TimedOut))?;
        stream.set_write_timeout(Some(remaining))?;
        match stream.write(remaining_bytes) {
            Ok(0) => return Err(std::io::Error::from(std::io::ErrorKind::WriteZero).into()),
            Ok(count) => remaining_bytes = &remaining_bytes[count..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
    if Instant::now() >= deadline {
        return Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into());
    }
    stream.shutdown(Shutdown::Both)?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Native profile: admission, Prepared delivery with EOF, full composition
/// quiescence and startup through the root execution helper, identity-fenced
/// readiness and confirmation, all under the same manager locks. The legacy
/// `Activate` request is refused; failures after admission leave the pending
/// operation to native recovery and never dispatch legacy apply services.
fn serve_native(
    config: &Path,
    mut stream: UnixStream,
    request: activation::ActivationRequestEnvelope,
) -> Result<(), SupervisorCliError> {
    use bli_kvm_update_runtime::native_exec_client::{EXECUTION_SOCKET, ExecutionClient};
    let request_id = request.request_id;
    let refuse = |stream: &mut UnixStream, error: SupervisorCliError| {
        let response = ActivationResponseEnvelope {
            schema_version: UPDATE_SCHEMA_VERSION,
            request_id,
            response: ActivationResponse::Error {
                code: error_code(&error),
            },
        };
        let _ = deliver_response_before(stream, &response, Instant::now() + Duration::from_secs(2));
        Err(error)
    };
    let ActivationRequest::Admit {
        slot_id,
        expected_generation,
    } = request.request
    else {
        return refuse(&mut stream, SupervisorCliError::ActivationNotDispatched);
    };
    let deadline = Instant::now() + Duration::from_secs(45);
    let supervisor = match open_supervisor(config) {
        Ok(supervisor) => supervisor,
        Err(error) => return refuse(&mut stream, error),
    };
    let mut client = match ExecutionClient::connect(Path::new(EXECUTION_SOCKET), deadline) {
        Ok(client) => client,
        Err(error) => return refuse(&mut stream, error.into()),
    };
    let probes = client.clone();
    let mut delivered = false;
    let mut admitted_operation = None;
    let admitted = supervisor.activate_native_candidate(
        &slot_id,
        expected_generation,
        &mut client,
        deadline,
        |admission| {
            let response = ActivationResponseEnvelope {
                schema_version: UPDATE_SCHEMA_VERSION,
                request_id,
                response: ActivationResponse::Prepared {
                    receipt: activation::PreparedActivationReceipt {
                        operation_id: hex(&admission.operation_id),
                        target_slot_id: admission.target_slot_id.clone(),
                        prepared_generation: admission.prepared_generation,
                        deadline_monotonic_ms: admission.deadline_monotonic_ms,
                    },
                },
            };
            // Quiescence stops the requesting server; it must hold the
            // complete receipt and EOF first, or admission is not delivered.
            deliver_response_before(
                &mut stream,
                &response,
                Instant::now() + Duration::from_secs(2),
            )
            .map_err(|_| UpdateManagerError::RecoveryRequired)?;
            delivered = true;
            admitted_operation = Some(admission.operation_id);
            Ok(())
        },
        |unit, _, _| probes.probe(unit),
    );
    let (activation, started) = match admitted {
        Ok(value) => value,
        Err(error) if !delivered => return refuse(&mut stream, error.into()),
        Err(error) => {
            // The requesting server is already stopped: restore the confirmed
            // composition now rather than leaving the board without a server.
            eprintln!("native activation failed after admission: {error:?}");
            drop((client, probes));
            restore_after_failure(&supervisor, admitted_operation);
            return Err(error.into());
        }
    };
    let ActivationOutcome::AwaitingHealth { operation_id, .. } = activation.outcome else {
        return Err(SupervisorCliError::ActivationNotDispatched);
    };
    let generation = supervisor.native_status()?.generation;
    let outcome = supervisor.confirm_native_candidate(
        generation,
        operation_id,
        &started,
        &mut client,
        deadline,
        |unit, _, _| probes.probe(unit),
    );
    drop((client, probes));
    match outcome {
        Ok(bli_kvm_update_manager::NativeConfirmationOutcome::Confirmed) => Ok(()),
        Ok(_) => {
            // A rejected candidate keeps running until the confirmed
            // composition is restored; do that now instead of at next boot.
            restore_after_failure(&supervisor, None);
            Err(SupervisorCliError::ActivationNotDispatched)
        }
        Err(error) => {
            eprintln!("native confirmation failed: {error:?}");
            restore_after_failure(&supervisor, Some(operation_id));
            Err(error.into())
        }
    }
}

/// Best-effort restoration of the confirmed native composition in a fresh
/// helper session after a failure past admission. `operation` is rejected
/// first when the journal still awaits its health; an existing rollback intent
/// is kept. Failures are logged; native boot remains the fallback.
fn restore_after_failure(supervisor: &UpdateSupervisor, operation: Option<[u8; 16]>) {
    use bli_kvm_update_runtime::native_exec_client::{EXECUTION_SOCKET, ExecutionClient};
    if let Some(operation_id) = operation
        && let Err(error) = supervisor.reject_native_candidate(operation_id)
    {
        eprintln!("native rejection not recorded: {error:?}");
    }
    let result = (|| -> Result<(), SupervisorCliError> {
        let generation = supervisor.inspect_recovery()?.generation;
        let deadline = Instant::now() + Duration::from_secs(45);
        let mut client = ExecutionClient::connect(Path::new(EXECUTION_SOCKET), deadline)?;
        let probes = client.clone();
        let outcome = supervisor.boot_native_composition(
            generation,
            &mut client,
            deadline,
            |unit, _, _| probes.probe(unit),
        )?;
        drop((client, probes));
        if matches!(outcome, bli_kvm_update_runtime::NativeBootOutcome::Ready(_)) {
            supervisor.archive_native_completion()?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => eprintln!("native composition restored after a failed activation"),
        Err(error) => eprintln!("native restoration after a failed activation failed: {error:?}"),
    }
}

fn open_supervisor(config: &Path) -> Result<UpdateSupervisor, SupervisorCliError> {
    let config = load_config(config)?;
    let manager = ApplicationUpdateManager::open(
        config.slot_root,
        config.state_root,
        config.trust_root,
        config.system_sequence,
        config.health_timeout_ms,
    )?;
    Ok(UpdateSupervisor::new(
        manager,
        HealthChallengeStore::open(config.runtime_root)?,
    ))
}

fn activate(
    config: &Path,
    slot: &str,
    generation: u64,
) -> Result<ActivationReceipt, SupervisorCliError> {
    let config = load_config(config)?;
    let manager = ApplicationUpdateManager::open(
        config.slot_root,
        config.state_root,
        config.trust_root,
        config.system_sequence,
        config.health_timeout_ms,
    )?;
    let supervisor =
        UpdateSupervisor::new(manager, HealthChallengeStore::open(config.runtime_root)?);
    let activation = supervisor.activate_if_generation(slot, generation)?;
    let ActivationOutcome::AwaitingHealth {
        operation_id,
        slot_id,
        ..
    } = activation.outcome
    else {
        return Err(SupervisorCliError::ActivationNotDispatched);
    };
    Ok(ActivationReceipt {
        operation_id: operation_id
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        target_slot_id: slot_id,
    })
}

fn error_code(error: &SupervisorCliError) -> ActivationErrorCode {
    match error {
        SupervisorCliError::Runtime(UpdateRuntimeError::RuntimeBusy) => ActivationErrorCode::Busy,
        SupervisorCliError::Runtime(UpdateRuntimeError::StaleActivationGeneration) => {
            ActivationErrorCode::StaleGeneration
        }
        SupervisorCliError::Runtime(UpdateRuntimeError::ActivationTargetAlreadyActive) => {
            ActivationErrorCode::AlreadyActive
        }
        SupervisorCliError::Runtime(UpdateRuntimeError::Manager(error))
        | SupervisorCliError::Manager(error) => match error {
            UpdateManagerError::PendingActivation => ActivationErrorCode::PendingActivation,
            UpdateManagerError::ManagerBusy
            | UpdateManagerError::Stager(ApplicationStageError::StagerBusy) => {
                ActivationErrorCode::Busy
            }
            UpdateManagerError::AntiRollback
            | UpdateManagerError::MinimumSystemSequence
            | UpdateManagerError::Stager(_) => ActivationErrorCode::InvalidTarget,
            _ => ActivationErrorCode::InvalidState,
        },
        _ => ActivationErrorCode::Unavailable,
    }
}

#[cfg(test)]
mod delivery_tests {
    use super::*;

    #[test]
    fn runtime_contention_is_retryable_busy_not_unavailable() {
        let error = SupervisorCliError::Runtime(UpdateRuntimeError::RuntimeBusy);
        assert_eq!(error_code(&error), ActivationErrorCode::Busy);
        assert_eq!(error.exit_code(), 66);
    }

    fn prepared() -> ActivationResponseEnvelope {
        ActivationResponseEnvelope {
            schema_version: UPDATE_SCHEMA_VERSION,
            request_id: 7,
            response: ActivationResponse::Prepared {
                receipt: activation::PreparedActivationReceipt {
                    operation_id: "1".repeat(32),
                    target_slot_id: format!("app-00000000000000000002-{}", "a".repeat(64)),
                    prepared_generation: 42,
                    deadline_monotonic_ms: 60_000,
                },
            },
        }
    }

    #[test]
    fn complete_receipt_publishes_eof_while_duplicate_descriptors_remain_open() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        let mut duplicate = writer.try_clone().unwrap();
        let response = prepared();
        deliver_response_before(
            &mut writer,
            &response,
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        reader
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut bytes = Vec::new();
        reader
            .take((MAX_UPDATE_MESSAGE_BYTES + 5) as u64)
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(
            u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize,
            bytes.len() - 4
        );
        assert_eq!(activation::decode_response(&bytes[4..]).unwrap(), response);
        assert!(duplicate.write_all(b"late reply").is_err());
    }

    #[test]
    fn closed_peer_and_expired_deadline_do_not_report_delivery() {
        let (mut writer, reader) = UnixStream::pair().unwrap();
        assert!(deliver_response_before(&mut writer, &prepared(), Instant::now()).is_err());
        drop(reader);
        assert!(
            deliver_response_before(
                &mut writer,
                &prepared(),
                Instant::now() + Duration::from_secs(1)
            )
            .is_err()
        );
    }

    #[test]
    fn nonreading_peer_is_bounded_by_the_total_delivery_deadline() {
        let (mut writer, _reader) = UnixStream::pair().unwrap();
        writer.set_nonblocking(true).unwrap();
        let mut blocked = false;
        for _ in 0..1024 {
            match writer.write(&[0_u8; 8192]) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    blocked = true;
                    break;
                }
                Err(error) => panic!("socket prefill failed: {error}"),
            }
        }
        assert!(blocked, "fixture socket did not reach backpressure");
        writer.set_nonblocking(false).unwrap();
        let start = Instant::now();
        assert!(
            deliver_response_before(&mut writer, &prepared(), start + Duration::from_millis(50))
                .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
