use std::{
    io::{Seek, Write},
    path::Path,
    time::Duration,
};

use bli_kvm_update_protocol::{
    ApplicationSlotVersion, UPDATE_SCHEMA_VERSION, encode_frame,
    upload::{
        UploadDisposition, UploadErrorCode, UploadReceipt, UploadRequest, UploadResponse,
        UploadResponseEnvelope, encode_response,
    },
};
use bli_kvm_update_stager::{
    ApplicationSlotStager, ApplicationStageError, StageDisposition, StageQuota,
};
use bli_kvm_update_supervisor::{
    upload_storage::{UploadStorage, check_staging_capacity},
    upload_transport::{ReceiveLimits, receive},
};

use crate::{SupervisorCliError, config::load_config, status_socket};

/// This entrypoint has no manager, journal, runtime or activation capability.
pub(crate) fn serve_stdin(
    config: &Path,
    peer_user: &str,
    spool: &Path,
) -> Result<(), SupervisorCliError> {
    // Authenticate before touching config, admission, metadata or body.
    let (mut stream, _) = status_socket::authorized_stdin(peer_user)?;
    let config = load_config(config)?;
    let storage = UploadStorage::open(spool)?;
    let (request, mut archive) = receive(&mut stream, ReceiveLimits::default(), |size| {
        storage.create_spool(size)
    })?;
    let UploadRequest::Stage { archive_bytes } = request.request;
    archive.rewind()?;
    let staged = (|| {
        let stager = ApplicationSlotStager::open(&config.slot_root)?;
        // The slot lock is acquired only after complete upload/EOF. The spool
        // remains accounted in f_bavail if both directories share a filesystem.
        check_staging_capacity(&config.slot_root, archive_bytes)
            .map_err(ApplicationStageError::Io)?;
        stager.stage_reader_with_quota(
            &mut archive,
            archive_bytes,
            &config.trust_root,
            StageQuota {
                max_slots: 8,
                max_payload_bytes: 512 * 1024 * 1024,
            },
        )
    })();
    let response = match staged {
        Ok(value) => UploadResponse::Staged {
            receipt: UploadReceipt {
                slot: ApplicationSlotVersion {
                    slot_id: value.slot_id,
                    version: value.manifest.version,
                    sequence: value.manifest.sequence,
                    minimum_system_sequence: value.manifest.minimum_system_sequence,
                },
                disposition: match value.disposition {
                    StageDisposition::Published => UploadDisposition::Published,
                    StageDisposition::AlreadyPresent => UploadDisposition::AlreadyPresent,
                },
                manifest_sha256: value.manifest_sha256,
                payload_bytes: value.payload_bytes,
            },
        },
        Err(error) => UploadResponse::Error {
            code: match error {
                ApplicationStageError::StagerBusy => UploadErrorCode::Busy,
                ApplicationStageError::QuotaExceeded => UploadErrorCode::QuotaExceeded,
                ApplicationStageError::Io(ref error)
                    if error.raw_os_error() == Some(libc::ENOSPC) =>
                {
                    UploadErrorCode::QuotaExceeded
                }
                ApplicationStageError::Verify(_)
                | ApplicationStageError::InvalidArchive
                | ApplicationStageError::ArchiveTooLarge
                | ApplicationStageError::ExtractionMismatch
                | ApplicationStageError::SourceChanged => UploadErrorCode::InvalidArchive,
                ApplicationStageError::Io(_) | ApplicationStageError::AtomicPublishUnavailable => {
                    UploadErrorCode::Unavailable
                }
                _ => UploadErrorCode::InvalidState,
            },
        },
    };
    let response = UploadResponseEnvelope {
        schema_version: UPDATE_SCHEMA_VERSION,
        request_id: request.request_id,
        response,
    };
    let bytes = encode_response(&response)
        .and_then(|bytes| encode_frame(&bytes))
        .map_err(status_socket::StatusSocketError::from)?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    // Loss of this response does not remove a published slot; retry is safe.
    stream.write_all(&bytes)?;
    Ok(())
}
