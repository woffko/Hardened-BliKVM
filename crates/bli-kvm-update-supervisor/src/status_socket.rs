use std::{
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    path::Path,
    time::Duration,
};

use bli_kvm_update_manager::{
    ApplicationUpdateManager, PendingPhaseStatus, RollbackReason, UpdateManagerError,
};
use bli_kvm_update_protocol::{
    ApplicationRollbackReason, ApplicationSlotVersion, ApplicationUpdateStatus,
    MAX_UPDATE_MESSAGE_BYTES, PendingApplicationUpdate, UPDATE_SCHEMA_VERSION, UpdatePendingPhase,
    UpdateProtocolError, UpdateReadErrorCode, UpdateReadRequest, UpdateReadResponse,
    UpdateResponseEnvelope, decode_request, encode_frame, encode_response,
};
use nix::unistd::User;
use thiserror::Error;

use crate::{SupervisorCliError, config::load_config};

#[derive(Debug, Error)]
pub(crate) enum StatusSocketError {
    #[error("status socket peer is not authorized")]
    PeerRejected,
    #[error("status socket I/O failed")]
    Io(#[from] std::io::Error),
    #[error("status socket protocol failed")]
    Protocol(#[from] UpdateProtocolError),
}

pub(crate) fn valid_peer_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value != "root"
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte))
        && value.as_bytes()[0].is_ascii_lowercase()
}

pub(crate) fn serve_stdin(config: &Path, peer_user: &str) -> Result<(), StatusSocketError> {
    if !valid_peer_name(peer_user) {
        return Err(StatusSocketError::PeerRejected);
    }
    let uid = User::from_name(peer_user)
        .map_err(|_| StatusSocketError::PeerRejected)?
        .ok_or(StatusSocketError::PeerRejected)?
        .uid
        .as_raw();
    if uid == 0 {
        return Err(StatusSocketError::PeerRejected);
    }
    // SAFETY: duplicates stdin without taking ownership of the original FD.
    let duplicate = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_DUPFD_CLOEXEC, 3) };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: fcntl returned a new uniquely owned descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(duplicate) };
    let mut stream = UnixStream::from(descriptor);
    serve_connection(&mut stream, uid, || read_status(config))
}

fn authorize(stream: &UnixStream, expected_uid: u32) -> Result<(), StatusSocketError> {
    let mut domain: libc::c_int = 0;
    let mut socket_type: libc::c_int = 0;
    for (option, value) in [
        (libc::SO_DOMAIN, &mut domain),
        (libc::SO_TYPE, &mut socket_type),
    ] {
        let mut size = size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: option expects an int; the borrowed FD and output storage live through this call.
        let result = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                option,
                (value as *mut libc::c_int).cast(),
                &mut size,
            )
        };
        if result != 0 || size as usize != size_of::<libc::c_int>() {
            return Err(StatusSocketError::PeerRejected);
        }
    }
    if domain != libc::AF_UNIX || socket_type != libc::SOCK_STREAM || expected_uid == 0 {
        return Err(StatusSocketError::PeerRejected);
    }
    let mut peer = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut size = size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: SO_PEERCRED initializes the correctly sized output on a connected Unix stream.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            peer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if result != 0 || size as usize != size_of::<libc::ucred>() {
        return Err(StatusSocketError::PeerRejected);
    }
    // SAFETY: the successful getsockopt call initialized all ucred fields.
    let peer = unsafe { peer.assume_init() };
    if peer.uid != expected_uid || peer.pid <= 0 {
        return Err(StatusSocketError::PeerRejected);
    }
    Ok(())
}

fn serve_connection(
    stream: &mut UnixStream,
    expected_uid: u32,
    status: impl FnOnce() -> Result<ApplicationUpdateStatus, SupervisorCliError>,
) -> Result<(), StatusSocketError> {
    authorize(stream, expected_uid)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_UPDATE_MESSAGE_BYTES {
        return Err(UpdateProtocolError::InvalidLength.into());
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    let request = decode_request(&bytes)?;
    let mut extra = [0_u8; 1];
    if stream.read(&mut extra)? != 0 {
        return Err(UpdateProtocolError::InvalidLength.into());
    }
    let UpdateReadRequest::Status {} = request.request;
    let response = match status() {
        Ok(value) => UpdateReadResponse::Status {
            status: Box::new(value),
        },
        Err(SupervisorCliError::Manager(UpdateManagerError::ManagerBusy)) => {
            UpdateReadResponse::Error {
                code: UpdateReadErrorCode::Busy,
            }
        }
        Err(SupervisorCliError::Manager(_) | SupervisorCliError::Stager(_)) => {
            UpdateReadResponse::Error {
                code: UpdateReadErrorCode::InvalidState,
            }
        }
        Err(_) => UpdateReadResponse::Error {
            code: UpdateReadErrorCode::Unavailable,
        },
    };
    let response = UpdateResponseEnvelope {
        schema_version: UPDATE_SCHEMA_VERSION,
        request_id: request.request_id,
        response,
    };
    stream.write_all(&encode_frame(&encode_response(&response)?)?)?;
    Ok(())
}

fn read_status(path: &Path) -> Result<ApplicationUpdateStatus, SupervisorCliError> {
    let config = load_config(path)?;
    let manager = ApplicationUpdateManager::open(
        config.slot_root.clone(),
        config.state_root,
        config.trust_root.clone(),
        config.system_sequence,
        config.health_timeout_ms,
    )?;
    let status = manager.status()?;
    let slot = |id: &str| -> Result<ApplicationSlotVersion, SupervisorCliError> {
        let verified = manager.inspect_slot(id)?;
        Ok(ApplicationSlotVersion {
            slot_id: verified.slot_id,
            version: verified.manifest.version,
            sequence: verified.manifest.sequence,
            minimum_system_sequence: verified.manifest.minimum_system_sequence,
        })
    };
    // The manager lock remains held until all referenced metadata is verified.
    // No health-store open, staging cleanup, acknowledgement or recovery runs here.
    Ok(ApplicationUpdateStatus {
        generation: status.generation,
        floor_sequence: status.floor_sequence,
        system_sequence: config.system_sequence,
        selected: slot(&status.active_slot_id)?,
        confirmed: slot(&status.confirmed_slot_id)?,
        previous: status.previous_slot_id.as_deref().map(slot).transpose()?,
        pending: status
            .pending
            .map(|pending| {
                Ok::<_, SupervisorCliError>(PendingApplicationUpdate {
                    operation_id: pending.operation_id,
                    target: slot(&pending.target_slot_id)?,
                    phase: match pending.phase {
                        PendingPhaseStatus::Prepared => UpdatePendingPhase::Prepared,
                        PendingPhaseStatus::AwaitingHealth => UpdatePendingPhase::AwaitingHealth,
                    },
                })
            })
            .transpose()?,
        last_rollback: status.last_rollback.map(|reason| match reason {
            RollbackReason::InterruptedPrepare => ApplicationRollbackReason::InterruptedPrepare,
            RollbackReason::BootChanged => ApplicationRollbackReason::BootChanged,
            RollbackReason::HealthTimeout => ApplicationRollbackReason::HealthTimeout,
            RollbackReason::ExplicitHealthFailure => {
                ApplicationRollbackReason::ExplicitHealthFailure
            }
            RollbackReason::PointerMismatch => ApplicationRollbackReason::PointerMismatch,
            RollbackReason::TargetInvalid => ApplicationRollbackReason::TargetInvalid,
            RollbackReason::HealthChallengeLost => ApplicationRollbackReason::HealthChallengeLost,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_update_protocol::{
        UpdateRequestEnvelope, decode_frame, decode_response, encode_request,
    };
    use std::{
        net::Shutdown,
        sync::atomic::{AtomicBool, Ordering},
    };

    #[test]
    fn peer_names_cannot_select_root_or_unbounded_identifiers() {
        assert!(valid_peer_name("hardened-blikvm-server"));
        for invalid in [
            "root",
            "",
            "user;command",
            "../user",
            "0user",
            &"a".repeat(33),
        ] {
            assert!(!valid_peer_name(invalid));
        }
    }

    #[test]
    fn unauthorized_peer_is_rejected_before_any_state_access() {
        let (_client, mut server) = UnixStream::pair().unwrap();
        let called = AtomicBool::new(false);
        let result = serve_connection(
            &mut server,
            nix::unistd::geteuid().as_raw().wrapping_add(1).max(1),
            || {
                called.store(true, Ordering::Relaxed);
                Err(SupervisorCliError::InvalidConfig)
            },
        );
        assert!(matches!(result, Err(StatusSocketError::PeerRejected)));
        assert!(!called.load(Ordering::Relaxed));
    }

    #[test]
    fn authorized_request_receives_only_a_correlated_coarse_error() {
        let uid = nix::unistd::geteuid().as_raw();
        let (mut client, mut server) = UnixStream::pair().unwrap();
        if uid == 0 {
            assert!(matches!(
                authorize(&server, uid),
                Err(StatusSocketError::PeerRejected)
            ));
            return;
        }
        let request = UpdateRequestEnvelope {
            schema_version: 1,
            request_id: 17,
            request: UpdateReadRequest::Status {},
        };
        client
            .write_all(&encode_frame(&encode_request(&request).unwrap()).unwrap())
            .unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        serve_connection(&mut server, uid, || Err(SupervisorCliError::InvalidConfig)).unwrap();
        drop(server);
        let mut bytes = Vec::new();
        client.read_to_end(&mut bytes).unwrap();
        let response = decode_response(decode_frame(&bytes).unwrap()).unwrap();
        assert_eq!(response.request_id, 17);
        assert_eq!(
            response.response,
            UpdateReadResponse::Error {
                code: UpdateReadErrorCode::Unavailable
            }
        );
    }
}
