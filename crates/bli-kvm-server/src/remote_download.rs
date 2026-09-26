//! Remote ISO download (reference `POST /api/download/image` and its status),
//! switched off by default.
//!
//! The server validates and forwards; the download helper, the only unit with
//! outbound network access, fetches the image, re-checks the switch and
//! reports progress through a status file this module reads.

use std::fs::File;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::{Duration, SystemTime};

use axum::{Json, extract::State};
use bli_kvm_admin_helper::{
    AdminReply, AdminRequest, DOWNLOAD_STATUS, DownloadStatus, validate_remote_iso,
};
use bli_kvm_core::AdminFeature;
use serde::Deserialize;

use crate::{
    AppState,
    error::{ApiResponse, AppError, AppResult},
};

/// A failed download is reported this long, then the status reads idle.
const FAILED_STATUS_TTL: Duration = Duration::from_secs(30);
const MAX_STATUS_BYTES: u64 = 4096;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DownloadBody {
    file: String,
}

pub(crate) async fn start(
    State(state): State<AppState>,
    Json(body): Json<DownloadBody>,
) -> AppResult<Json<ApiResponse<DownloadStatus>>> {
    crate::admin_features::require(AdminFeature::RemoteDownload)?;
    let socket = state.options.download_socket.as_deref().ok_or_else(|| {
        AppError::ServiceUnavailable("the download helper is not configured".into())
    })?;
    let iso = validate_remote_iso(&body.file)
        .ok_or_else(|| AppError::BadRequest("invalid url".into()))?;
    let request = AdminRequest::ImageDownload {
        url: iso.url.clone(),
    };
    match crate::scripts::call(socket, &request, None).await? {
        AdminReply::Ready => Ok(Json(ApiResponse::ok(DownloadStatus {
            status: "in_progress".into(),
            file: iso.url,
            percentage: String::new(),
        }))),
        AdminReply::Refused { reason } => Err(AppError::BadRequest(reason)),
        _ => Err(AppError::ServiceUnavailable(
            "unexpected download helper answer".into(),
        )),
    }
}

pub(crate) async fn status() -> Json<ApiResponse<DownloadStatus>> {
    Json(ApiResponse::ok(read_status(Path::new(DOWNLOAD_STATUS))))
}

/// True while the download helper runs a job (an image or the PicoClaw
/// runtime).
pub(crate) fn download_running() -> bool {
    lock_held(Path::new(DOWNLOAD_STATUS))
}

/// True while the helper holds the download lock next to the status file.
fn lock_held(status_path: &Path) -> bool {
    let Ok(file) = File::open(status_path.with_file_name("download.lock")) else {
        return false;
    };
    // SAFETY: flock on a descriptor this function owns; a shared lock that
    // succeeds is released when the file closes.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) != 0 }
}

/// The helper's status, or idle when there is none, when a failure is old
/// or when an `in_progress` status has no running download behind it.
pub(crate) fn read_status(path: &Path) -> DownloadStatus {
    let Ok(file) = File::open(path) else {
        return DownloadStatus::idle();
    };
    let modified = file
        .metadata()
        .and_then(|metadata| metadata.modified())
        .ok();
    let mut bytes = Vec::new();
    if file.take(MAX_STATUS_BYTES).read_to_end(&mut bytes).is_err() {
        return DownloadStatus::idle();
    }
    let Ok(status) = serde_json::from_slice::<DownloadStatus>(&bytes) else {
        return DownloadStatus::idle();
    };
    let age = modified
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .unwrap_or_default();
    match status.status.as_str() {
        "in_progress" if lock_held(path) => status,
        "failed" if age < FAILED_STATUS_TTL => status,
        _ => DownloadStatus::idle(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn status_reads_idle_unless_a_download_runs_or_just_failed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        assert_eq!(read_status(&path), DownloadStatus::idle());
        let running = DownloadStatus {
            status: "in_progress".into(),
            file: "https://mirror.example/a.iso".into(),
            percentage: "12.00%".into(),
        };
        fs::write(&path, serde_json::to_vec(&running).unwrap()).unwrap();
        // Nobody holds the lock: a stale status after a helper crash.
        assert_eq!(read_status(&path), DownloadStatus::idle());
        let lock = File::create(dir.path().join("download.lock")).unwrap();
        // SAFETY: flock on an open descriptor owned by the test.
        assert_eq!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        assert_eq!(read_status(&path), running);
        let failed = DownloadStatus {
            status: "failed".into(),
            ..running
        };
        fs::write(&path, serde_json::to_vec(&failed).unwrap()).unwrap();
        assert_eq!(read_status(&path), failed);
        fs::write(&path, b"{not json").unwrap();
        assert_eq!(read_status(&path), DownloadStatus::idle());
    }
}
