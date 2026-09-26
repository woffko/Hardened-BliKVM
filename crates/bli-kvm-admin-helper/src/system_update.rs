//! RAW system update requests (`docs/design/system-raw-update-v1.md`).
//!
//! The helper stores an uploaded bundle on the data partition, verifies it
//! with the system trust root the update-runtime package ships and stages it
//! for the boot-time step. Every request except the status and cancelling a
//! staged update needs the operator's switch; the caller checks it.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use bli_kvm_admin_helper::{FRAME_DATA, MAX_SYSTEM_BUNDLE_BYTES, SystemUpdateState, read_frame};
use bli_kvm_system_image_verifier::SystemVerificationPolicy;
use bli_kvm_system_raw_update::{
    Phase, Record, StageRequest, SystemTools, UpdateError, cancel, confirm, load_trust,
    minimum_sequence, read_record, request_rollback, stage, staging_path,
};

pub const TRUST_FILE: &str = "/usr/share/hardened-blikvm/system-update/system-trust.json";
const DATA_MOUNT: &str = "/mnt";
const UPLOAD_FILE: &str = "upload.tar";
/// Room kept free on the data partition beyond the update's own files.
const SPARE_BYTES: u64 = 256 * 1024 * 1024;

pub fn staging() -> PathBuf {
    staging_path(Path::new(DATA_MOUNT))
}

fn error(error: UpdateError) -> io::Error {
    match error {
        UpdateError::Io(error) => error,
        other => io::Error::other(other.to_string()),
    }
}

fn view(staging: &Path, record: Option<Record>) -> SystemUpdateState {
    let minimum_sequence = minimum_sequence(staging);
    match record {
        Some(record) => SystemUpdateState {
            phase: serde_json::to_value(record.phase)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default(),
            version: record.version,
            sequence: record.sequence,
            message: record.message,
            boot_attempts: record.boot_attempts,
            image_size: record.image_size,
            minimum_sequence,
        },
        None => SystemUpdateState {
            phase: "none".into(),
            minimum_sequence,
            ..SystemUpdateState::default()
        },
    }
}

pub fn status() -> io::Result<SystemUpdateState> {
    let staging = staging();
    let record = read_record(&staging).map_err(error)?;
    Ok(view(&staging, record))
}

/// Bytes free for root on the file system holding `path`, and its size in
/// use.
fn space(path: &Path) -> io::Result<(u64, u64)> {
    let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| io::Error::other("path"))?;
    // SAFETY: `statvfs` only writes into the zeroed structure.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let block = stats.f_frsize;
    Ok((
        stats.f_bfree.saturating_mul(block),
        stats
            .f_blocks
            .saturating_sub(stats.f_bfree)
            .saturating_mul(block),
    ))
}

/// The staging directory, on a data partition separate from the root.
fn prepare_staging() -> io::Result<PathBuf> {
    let root = fs::metadata("/")?;
    let data = fs::metadata(DATA_MOUNT)?;
    if root.dev() == data.dev() {
        return Err(io::Error::other(
            "the data partition is not mounted at /mnt",
        ));
    }
    let staging = staging();
    match fs::symlink_metadata(&staging) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(io::Error::other("the staging path is not a directory")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new().mode(0o700).create(&staging)?;
        }
        Err(error) => return Err(error),
    }
    Ok(staging)
}

/// Checks that an upload of `size` bytes can be staged and applied.
pub fn check_upload(size: u64) -> io::Result<()> {
    if !(1..=MAX_SYSTEM_BUNDLE_BYTES).contains(&size) {
        return Err(io::Error::other("the system bundle size is not accepted"));
    }
    let staging = prepare_staging()?;
    if let Some(record) = read_record(&staging).map_err(error)?
        && matches!(
            record.phase,
            Phase::Writing | Phase::Written | Phase::Booted | Phase::RollbackRequested
        )
    {
        return Err(io::Error::other(
            "a system update is in progress; confirm or roll it back first",
        ));
    }
    // The upload and the staged payload exist together, then the payload
    // and the root backup; the backup is at most the root's used size once
    // compressed in practice, and the boot-time step refuses cleanly if it
    // does not fit.
    let (free, _) = space(&staging)?;
    let (_, root_used) = space(Path::new("/"))?;
    let needed = size
        .saturating_add(size.max(root_used))
        .saturating_add(SPARE_BYTES);
    if free < needed {
        return Err(io::Error::other(format!(
            "the data partition needs {} MiB free for this update",
            needed / (1024 * 1024)
        )));
    }
    Ok(())
}

/// Receives the bundle frames into the staging directory.
pub fn receive(reader: &mut impl io::Read, size: u64) -> io::Result<PathBuf> {
    let upload = staging().join(UPLOAD_FILE);
    let mut out = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&upload)?;
    let mut received = 0_u64;
    let result = (|| -> io::Result<()> {
        while received < size {
            match read_frame(reader)? {
                Some((FRAME_DATA, payload))
                    if received + payload.len() as u64 <= size && !payload.is_empty() =>
                {
                    out.write_all(&payload)?;
                    received += payload.len() as u64;
                }
                _ => return Err(io::Error::other("the system bundle upload is incomplete")),
            }
        }
        out.sync_all()
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&upload);
        return Err(error);
    }
    Ok(upload)
}

fn root_filesystem() -> io::Result<(PathBuf, String)> {
    let output = Command::new("/usr/bin/findmnt")
        .args([
            "--noheadings",
            "--output",
            "SOURCE,UUID",
            "--mountpoint",
            "/",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut fields = text.split_whitespace();
    match (fields.next(), fields.next()) {
        (Some(source), Some(uuid)) if output.status.success() && source.starts_with("/dev/") => {
            Ok((PathBuf::from(source), uuid.to_string()))
        }
        _ => Err(io::Error::other("the root partition is unknown")),
    }
}

/// Verifies and stages an uploaded bundle; the upload is removed either way.
pub fn stage_upload(upload: &Path) -> io::Result<SystemUpdateState> {
    let staging = staging();
    let result = (|| -> io::Result<Record> {
        let (trust, layout_id) = load_trust(Path::new(TRUST_FILE)).map_err(error)?;
        let (device, uuid) = root_filesystem()?;
        let partition = {
            let mut file = File::open(&device)?;
            io::Seek::seek(&mut file, io::SeekFrom::End(0))?
        };
        let policy = SystemVerificationPolicy::new(layout_id, 1, 1, partition, None)
            .map_err(|error| io::Error::other(error.to_string()))?;
        stage(
            &StageRequest {
                bundle: upload,
                staging: &staging,
                trust: &trust,
                policy: &policy,
                minimum_sequence: minimum_sequence(&staging),
                root: Path::new("/"),
                root_uuid: &uuid,
                fstab: Path::new("/etc/fstab"),
                shadow: Path::new("/etc/shadow"),
            },
            &mut SystemTools,
        )
        .map_err(error)
    })();
    let _ = fs::remove_file(upload);
    let record = result?;
    eprintln!(
        "admin helper: system update {} (sequence {}) staged",
        record.version, record.sequence
    );
    Ok(view(&staging, Some(record)))
}

pub fn confirm_update() -> io::Result<SystemUpdateState> {
    let staging = staging();
    let record = confirm(&staging).map_err(error)?;
    eprintln!("admin helper: system update {} confirmed", record.version);
    Ok(view(&staging, Some(record)))
}

pub fn rollback_update() -> io::Result<SystemUpdateState> {
    let staging = staging();
    let record = request_rollback(&staging).map_err(error)?;
    eprintln!(
        "admin helper: system update {} rollback requested",
        record.version
    );
    Ok(view(&staging, Some(record)))
}

pub fn cancel_update() -> io::Result<SystemUpdateState> {
    let staging = staging();
    cancel(&staging).map_err(error)?;
    eprintln!("admin helper: staged system update cancelled");
    status()
}
