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

pub(crate) fn serve_stdin(config: &Path, peer_user: &str) -> Result<(), SupervisorCliError> {
    let (mut stream, _) = status_socket::authorized_stdin(peer_user)?;
    // Authenticate and fully close the bounded request before taking any
    // journal/slot locks. A slow sender cannot block watchdog or status.
    let request = receive(&mut stream)?;
    let ActivationRequest::Activate {
        slot_id,
        expected_generation,
    } = request.request;
    let result = activate(config, &slot_id, expected_generation);
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
    let delivery = (|| -> Result<(), StatusSocketError> {
        let bytes = encode_frame(&activation::encode_response(&response)?)?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        stream.write_all(&bytes)?;
        Ok(())
    })();
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
