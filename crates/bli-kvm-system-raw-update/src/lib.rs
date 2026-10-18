//! RAW system update for the BliKVM card layout (`docs/design/system-raw-update-v1.md`).
//!
//! `stage` verifies a signed system bundle, copies its compressed root image
//! to the data partition, archives the device state that must survive, and
//! writes the apply record. `apply` runs from the initramfs before the root
//! file system is mounted: it backs the current root up, writes the new image
//! while hashing it, reads it back, grows it to the partition, gives it the
//! old file-system identity, points its boot configuration at it and puts the
//! carried state back, all before the new root first boots. A failure
//! restores the backup. `mark_booted` and `confirm` finish the update, and a
//! new root that does not report itself booted within the allowed boot
//! attempts is replaced by the backup again.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use bli_kvm_system_image_verifier::{
    SystemContentType, SystemTrustRoot, SystemVerificationPolicy, verify_system_file,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const RECORD_FILE: &str = "apply.json";
pub const PAYLOAD_FILE: &str = "payload.img.zst";
pub const STATE_FILE: &str = "state.tar";
pub const BACKUP_FILE: &str = "root.backup.zst";
pub const FSTAB_FILE: &str = "fstab";
/// The device's `/etc/shadow`, merged into the new root per user so the
/// image's default passwords never replace the operator's.
pub const SHADOW_FILE: &str = "shadow";
/// The board's accounts and groups, kept at stage to check the new image's
/// against before the carried state is restored with its numeric owners.
pub const PASSWD_FILE: &str = "passwd";
pub const GROUP_FILE: &str = "group";
/// The sequence of the last confirmed system bundle; older ones are refused.
pub const CONFIRMED_FILE: &str = "confirmed-sequence";
/// A new root that has not reported itself booted after this many boots is
/// replaced by the backup.
pub const MAX_BOOT_ATTEMPTS: u32 = 2;
const RECORD_SCHEMA: u32 = 1;
const MAX_RECORD_BYTES: u64 = 64 * 1024;
const BUFFER: usize = 1024 * 1024;

/// The device state carried across a root write, relative to `/`. Paths that
/// do not exist on a device are skipped.
pub const CARRIED_STATE: &[&str] = &[
    "etc/hardened-blikvm",
    "etc/hardened-blikvm-gadget",
    "etc/ssh/ssh_host_dsa_key",
    "etc/ssh/ssh_host_dsa_key.pub",
    "etc/ssh/ssh_host_ecdsa_key",
    "etc/ssh/ssh_host_ecdsa_key.pub",
    "etc/ssh/ssh_host_ed25519_key",
    "etc/ssh/ssh_host_ed25519_key.pub",
    "etc/ssh/ssh_host_rsa_key",
    "etc/ssh/ssh_host_rsa_key.pub",
    "etc/machine-id",
    "etc/hostname",
    "etc/hosts",
    "etc/NetworkManager/system-connections",
    // The stable-privacy IPv6 addresses and the DHCP client identity derive
    // from this key; the image leaves it out, so without it every update
    // would give the board new addresses.
    "var/lib/NetworkManager/secret_key",
    "root/.ssh",
    "var/lib/hardened-blikvm",
    // The application's update state: the journal with its floor and
    // confirmed slot, the slot pointer and the protected copies' selection,
    // so the board keeps its own application across the write.
    "var/lib/hardened-blikvm-update",
    "var/lib/hardened-blikvm-update-v2",
    "var/lib/hardened-blikvm-exec",
    "var/lib/hardened-blikvm-exec-ledger",
    "var/lib/hardened-blikvm-slots",
    "var/lib/hardened-blikvm-slots-v2",
    "var/lib/hardened-blikvm-gadget",
    "var/lib/hardened-blikvm-system-actions",
    "var/lib/hardened-blikvm-video",
    "var/lib/hardened-blikvm-admin",
    "var/lib/private/hardened-blikvm-picoclaw",
    "var/lib/hardened-blikvm-picoclaw-runtime",
    "var/lib/tailscale",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Staged; the next boot writes it.
    Staged,
    /// The write started; the backup is complete.
    Writing,
    /// Written, verified and given the carried state; waiting for the new
    /// root to report itself.
    Written,
    /// The new root is up and healthy; waiting for the operator.
    Booted,
    /// The operator asked for the backup; the next boot writes it back.
    RollbackRequested,
    /// Kept by the operator; the backup is gone.
    Confirmed,
    /// The backup was written back.
    RolledBack,
    /// Nothing was written; the old root is untouched.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub schema: u32,
    pub generation: u64,
    pub phase: Phase,
    pub version: String,
    pub sequence: u64,
    pub manifest_sha256: String,
    pub payload_size: u64,
    pub payload_sha256: String,
    pub image_size: u64,
    pub image_sha256: String,
    /// The root file system's UUID, which the boot configuration names.
    pub root_uuid: String,
    #[serde(default)]
    pub backup_size: u64,
    #[serde(default)]
    pub backup_sha256: String,
    #[serde(default)]
    pub boot_attempts: u32,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("the system bundle is not valid: {0}")]
    Bundle(String),
    #[error("an update is already in progress")]
    Busy,
    #[error("no update is waiting for this step")]
    NothingToDo,
    #[error("{0}")]
    Check(String),
    #[error("I/O failed: {0}")]
    Io(#[from] io::Error),
}

pub type Result<T> = std::result::Result<T, UpdateError>;

/// Privileged steps the host tests replace.
pub trait Tools {
    /// Streams `zstd -dc payload` into `sink`.
    fn decompress(
        &mut self,
        payload: &Path,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<()>;
    /// Compresses `source` (the first `size` bytes) into `target`.
    fn compress(&mut self, source: &Path, size: u64, target: &Path) -> io::Result<()>;
    /// `resize2fs` to the partition and `tune2fs -U` to the old identity.
    fn finish_filesystem(&mut self, device: &Path, uuid: &str) -> io::Result<()>;
    /// Mounts the new root, points `/boot/armbianEnv.txt` and the data and
    /// media lines of `/etc/fstab` at this device, merges the device's
    /// password hashes and extracts the carried state archive over it; the
    /// device's files are in `staging`.
    fn prepare_root(&mut self, device: &Path, uuid: &str, staging: &Path) -> io::Result<()>;
    /// Archives the carried state of `root` into `target`.
    fn archive_state(&mut self, root: &Path, paths: &[&str], target: &Path) -> io::Result<()>;
    /// Extracts `archive` over `root`, keeping owners and modes.
    fn restore_state(&mut self, root: &Path, archive: &Path) -> io::Result<()>;
}

pub fn read_record(staging: &Path) -> Result<Option<Record>> {
    let path = staging.join(RECORD_FILE);
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(UpdateError::Check("the apply record is too large".into()));
    }
    let record: Record = serde_json::from_slice(&bytes)
        .map_err(|_| UpdateError::Check("the apply record is invalid".into()))?;
    if record.schema != RECORD_SCHEMA {
        return Err(UpdateError::Check(
            "the apply record schema is unknown".into(),
        ));
    }
    Ok(Some(record))
}

/// Replaces the record atomically and durably.
pub fn write_record(staging: &Path, record: &Record) -> Result<()> {
    let path = staging.join(RECORD_FILE);
    let temporary = staging.join(".apply.json.new");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(record).map_err(io::Error::other)?)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, &path)?;
    File::open(staging)?.sync_all()?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Size and SHA-256 of the first `limit` bytes of `path` (all when `None`).
pub fn digest(path: &Path, limit: Option<u64>) -> io::Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER];
    let mut total = 0_u64;
    loop {
        let want = match limit {
            Some(limit) => (limit - total).min(BUFFER as u64) as usize,
            None => BUFFER,
        };
        if want == 0 {
            break;
        }
        let read = file.read(&mut buffer[..want])?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total += read as u64;
    }
    Ok((total, hex(&hasher.finalize())))
}

/// Copies `source` to `target` as a root-only file.
fn copy_private(source: &Path, target: &Path) -> io::Result<()> {
    let bytes = fs::read(source)?;
    let mut out = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(target)?;
    out.write_all(&bytes)?;
    out.sync_all()
}

pub struct StageRequest<'a> {
    pub bundle: &'a Path,
    pub staging: &'a Path,
    pub trust: &'a SystemTrustRoot,
    pub policy: &'a SystemVerificationPolicy,
    /// Bundles below this sequence are refused.
    pub minimum_sequence: u64,
    pub root: &'a Path,
    pub root_uuid: &'a str,
    /// The current `/etc/fstab`, kept for the new root's data and media lines.
    pub fstab: &'a Path,
    /// The current `/etc/shadow`.
    pub shadow: &'a Path,
    /// The current `/etc/passwd` and `/etc/group`.
    pub passwd: &'a Path,
    pub group: &'a Path,
}

/// Verifies the bundle and prepares the next boot's write.
pub fn stage(request: &StageRequest<'_>, tools: &mut dyn Tools) -> Result<Record> {
    if let Some(record) = read_record(request.staging)?
        && matches!(
            record.phase,
            Phase::Writing | Phase::Written | Phase::Booted | Phase::RollbackRequested
        )
    {
        return Err(UpdateError::Busy);
    }
    let previous_generation = read_record(request.staging)?
        .map(|record| record.generation)
        .unwrap_or(0);
    let verified = verify_system_file(request.bundle, request.trust, request.policy)
        .map_err(|error| UpdateError::Bundle(error.to_string()))?;
    let manifest = verified.manifest;
    if manifest.sequence < request.minimum_sequence {
        return Err(UpdateError::Bundle(format!(
            "sequence {} is below the accepted {}",
            manifest.sequence, request.minimum_sequence
        )));
    }
    let component = manifest
        .components
        .iter()
        .find(|component| component.content_type == SystemContentType::Ext4Zstd)
        .ok_or_else(|| UpdateError::Bundle("the bundle has no compressed root image".into()))?
        .clone();
    let staged = (|| -> Result<Record> {
        // Copy the payload out of the verified archive and hash it again: the
        // archive could change between the two reads.
        let payload = request.staging.join(PAYLOAD_FILE);
        let temporary = request.staging.join(".payload.img.zst.new");
        let mut archive = tar::Archive::new(File::open(request.bundle)?);
        let mut copied = None;
        for entry in archive.entries()? {
            let mut entry = entry?;
            if entry.path_bytes().as_ref() != format!("payload/{}", component.path).as_bytes() {
                continue;
            }
            let mut out = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&temporary)?;
            let mut hasher = Sha256::new();
            let mut buffer = vec![0_u8; BUFFER];
            let mut total = 0_u64;
            loop {
                let read = entry.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
                out.write_all(&buffer[..read])?;
                total += read as u64;
            }
            out.sync_all()?;
            copied = Some((total, hex(&hasher.finalize())));
            break;
        }
        if copied != Some((component.size, component.sha256.clone())) {
            let _ = fs::remove_file(&temporary);
            return Err(UpdateError::Bundle(
                "the payload changed while staging".into(),
            ));
        }
        fs::rename(&temporary, &payload)?;
        tools.archive_state(
            request.root,
            CARRIED_STATE,
            &request.staging.join(STATE_FILE),
        )?;
        fs::copy(request.fstab, request.staging.join(FSTAB_FILE))?;
        copy_private(request.shadow, &request.staging.join(SHADOW_FILE))?;
        fs::copy(request.passwd, request.staging.join(PASSWD_FILE))?;
        fs::copy(request.group, request.staging.join(GROUP_FILE))?;
        let record = Record {
            schema: RECORD_SCHEMA,
            generation: previous_generation + 1,
            phase: Phase::Staged,
            version: manifest.version.clone(),
            sequence: manifest.sequence,
            manifest_sha256: verified.manifest_sha256,
            payload_size: component.size,
            payload_sha256: component.sha256,
            image_size: component.image_size.unwrap_or(0),
            image_sha256: component.image_sha256.unwrap_or_default(),
            root_uuid: request.root_uuid.to_string(),
            backup_size: 0,
            backup_sha256: String::new(),
            boot_attempts: 0,
            message: "staged; reboot to write".into(),
        };
        write_record(request.staging, &record)?;
        Ok(record)
    })();
    if staged.is_err() {
        // Nothing of a failed stage stays to fill the data partition or to
        // be taken for a staged update; a record that was staged before
        // names files that are gone now (security review of 2026-10-18, N4).
        discard(request.staging);
        for leftover in [".payload.img.zst.new", "state.tar.new"] {
            let _ = fs::remove_file(request.staging.join(leftover));
        }
        if let Ok(Some(mut earlier)) = read_record(request.staging)
            && earlier.phase == Phase::Staged
        {
            earlier.phase = Phase::Failed;
            earlier.message = "a later staging failed; nothing was written".into();
            let _ = write_record(request.staging, &earlier);
        }
    }
    staged
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootAction {
    /// Nothing to do; continue booting.
    Continue,
    /// The root changed; reboot so the boot loader reads its configuration.
    Reboot,
}

fn size_of(device: &Path) -> io::Result<u64> {
    let mut file = File::open(device)?;
    io::Seek::seek(&mut file, io::SeekFrom::End(0))
}

/// Writes the decompressed payload onto `device`, hashing what it writes.
fn write_image(
    tools: &mut dyn Tools,
    payload: &Path,
    device: &Path,
    size: u64,
    sha256: &str,
) -> io::Result<()> {
    let mut out = OpenOptions::new().write(true).open(device)?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    tools.decompress(payload, &mut |chunk| {
        total += chunk.len() as u64;
        if total > size {
            return Err(io::Error::other("the image is larger than signed"));
        }
        hasher.update(chunk);
        out.write_all(chunk)
    })?;
    out.sync_all()?;
    if total != size || hex(&hasher.finalize()) != sha256 {
        return Err(io::Error::other(
            "the written image does not match the signed digest",
        ));
    }
    let (read, digest) = digest(device, Some(size))?;
    if read != size || digest != sha256 {
        return Err(io::Error::other("the partition read back differently"));
    }
    Ok(())
}

/// Writes the backup back onto `device` and checks it.
fn write_backup(
    tools: &mut dyn Tools,
    record: &Record,
    staging: &Path,
    device: &Path,
) -> io::Result<()> {
    write_image(
        tools,
        &staging.join(BACKUP_FILE),
        device,
        record.backup_size,
        &record.backup_sha256,
    )
}

/// The initramfs step, before the root is mounted.
pub fn boot(staging: &Path, device: &Path, tools: &mut dyn Tools) -> Result<BootAction> {
    let Some(mut record) = read_record(staging)? else {
        return Ok(BootAction::Continue);
    };
    match record.phase {
        Phase::Staged => apply(&mut record, staging, device, tools),
        Phase::Writing => {
            // Power was lost during the write: the backup is complete.
            record.message = "the write was interrupted; the backup was restored".into();
            restore_backup(&mut record, staging, device, tools)
        }
        Phase::Written => {
            record.boot_attempts += 1;
            if record.boot_attempts > MAX_BOOT_ATTEMPTS {
                record.message = format!(
                    "the new root did not come up in {MAX_BOOT_ATTEMPTS} boots; the backup was restored"
                );
                return restore_backup(&mut record, staging, device, tools);
            }
            write_record(staging, &record)?;
            Ok(BootAction::Continue)
        }
        Phase::RollbackRequested => {
            record.message = "the backup was restored on request".into();
            restore_backup(&mut record, staging, device, tools)
        }
        Phase::Booted | Phase::Confirmed | Phase::RolledBack | Phase::Failed => {
            Ok(BootAction::Continue)
        }
    }
}

fn apply(
    record: &mut Record,
    staging: &Path,
    device: &Path,
    tools: &mut dyn Tools,
) -> Result<BootAction> {
    let payload = staging.join(PAYLOAD_FILE);
    let checks = (|| -> io::Result<()> {
        if digest(&payload, None)? != (record.payload_size, record.payload_sha256.clone()) {
            return Err(io::Error::other("the staged payload changed"));
        }
        if record.image_size > size_of(device)? {
            return Err(io::Error::other("the image does not fit the partition"));
        }
        Ok(())
    })();
    if let Err(error) = checks {
        record.phase = Phase::Failed;
        record.message = format!("not written: {error}");
        write_record(staging, record)?;
        discard(staging);
        return Ok(BootAction::Continue);
    }
    // Back up the whole partition and prove the backup before touching it.
    let backup = (|| -> io::Result<(u64, String)> {
        let partition = size_of(device)?;
        let original = digest(device, Some(partition))?;
        tools.compress(device, partition, &staging.join(BACKUP_FILE))?;
        let mut hasher = Sha256::new();
        let mut total = 0_u64;
        tools.decompress(&staging.join(BACKUP_FILE), &mut |chunk| {
            total += chunk.len() as u64;
            hasher.update(chunk);
            Ok(())
        })?;
        if (total, hex(&hasher.finalize())) != original {
            return Err(io::Error::other("the backup did not read back identically"));
        }
        Ok(original)
    })();
    let (backup_size, backup_sha256) = match backup {
        Ok(backup) => backup,
        Err(error) => {
            record.phase = Phase::Failed;
            record.message = format!("not written: the backup failed: {error}");
            write_record(staging, record)?;
            discard(staging);
            return Ok(BootAction::Continue);
        }
    };
    record.backup_size = backup_size;
    record.backup_sha256 = backup_sha256;
    record.phase = Phase::Writing;
    record.message = "writing".into();
    write_record(staging, record)?;
    let written = write_image(
        tools,
        &payload,
        device,
        record.image_size,
        &record.image_sha256,
    )
    .and_then(|()| tools.finish_filesystem(device, &record.root_uuid))
    .and_then(|()| tools.prepare_root(device, &record.root_uuid, staging));
    match written {
        Ok(()) => {
            record.phase = Phase::Written;
            record.boot_attempts = 0;
            record.message = "written; booting the new root".into();
            write_record(staging, record)?;
            Ok(BootAction::Reboot)
        }
        Err(error) => {
            record.message = format!("the write failed ({error}); the backup was restored");
            restore_backup(record, staging, device, tools)
        }
    }
}

fn restore_backup(
    record: &mut Record,
    staging: &Path,
    device: &Path,
    tools: &mut dyn Tools,
) -> Result<BootAction> {
    write_backup(tools, record, staging, device)?;
    record.phase = Phase::RolledBack;
    write_record(staging, record)?;
    // The backup is the root again; nothing staged can be used any more.
    discard(staging);
    Ok(BootAction::Reboot)
}

/// Removes the staged payload, the backup, the carried state and the copies
/// of the device's fstab and shadow once no step can use them.
fn discard(staging: &Path) {
    for file in [
        BACKUP_FILE,
        PAYLOAD_FILE,
        STATE_FILE,
        FSTAB_FILE,
        SHADOW_FILE,
        PASSWD_FILE,
        GROUP_FILE,
    ] {
        let _ = fs::remove_file(staging.join(file));
    }
}

/// The new root came up healthy.
pub fn mark_booted(staging: &Path) -> Result<Record> {
    let mut record = read_record(staging)?.ok_or(UpdateError::NothingToDo)?;
    if record.phase != Phase::Written {
        return Err(UpdateError::NothingToDo);
    }
    record.phase = Phase::Booted;
    record.message = "the new system is running; confirm to keep it".into();
    write_record(staging, &record)?;
    Ok(record)
}

/// The operator keeps the new system; the staged files go.
pub fn confirm(staging: &Path) -> Result<Record> {
    let mut record = read_record(staging)?.ok_or(UpdateError::NothingToDo)?;
    if record.phase != Phase::Booted {
        return Err(UpdateError::NothingToDo);
    }
    for file in [
        BACKUP_FILE,
        PAYLOAD_FILE,
        STATE_FILE,
        FSTAB_FILE,
        SHADOW_FILE,
    ] {
        match fs::remove_file(staging.join(file)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let temporary = staging.join(".confirmed-sequence.new");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    writeln!(file, "{}", record.sequence)?;
    file.sync_all()?;
    fs::rename(&temporary, staging.join(CONFIRMED_FILE))?;
    record.phase = Phase::Confirmed;
    record.message = "the new system is kept".into();
    write_record(staging, &record)?;
    Ok(record)
}

/// The operator wants the old system back: the next boot restores the backup.
pub fn request_rollback(staging: &Path) -> Result<Record> {
    let mut record = read_record(staging)?.ok_or(UpdateError::NothingToDo)?;
    if record.phase != Phase::Booted {
        return Err(UpdateError::NothingToDo);
    }
    record.phase = Phase::RollbackRequested;
    record.message = "reboot to restore the previous system".into();
    write_record(staging, &record)?;
    Ok(record)
}

/// The lowest sequence the device accepts: one above the last confirmed.
pub fn minimum_sequence(staging: &Path) -> u64 {
    fs::read_to_string(staging.join(CONFIRMED_FILE))
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .map_or(1, |sequence| sequence.saturating_add(1))
}

/// Drops a staged update that has not been written yet.
/// Also removes what a finished update left behind (rolled back or failed,
/// for example by an older boot step that did not clean up).
pub fn cancel(staging: &Path) -> Result<()> {
    let mut record = read_record(staging)?.ok_or(UpdateError::NothingToDo)?;
    match record.phase {
        Phase::Staged => {
            discard(staging);
            record.phase = Phase::Failed;
            record.message = "the staged update was cancelled".into();
            write_record(staging, &record)?;
            Ok(())
        }
        Phase::RolledBack | Phase::Failed => {
            discard(staging);
            Ok(())
        }
        _ => Err(UpdateError::NothingToDo),
    }
}

/// Rewrites the data and media lines of `new_fstab` with the device's own
/// ones (same mount point) and the root line with `root_uuid`.
pub fn merge_fstab(new_fstab: &str, device_fstab: &str, root_uuid: &str) -> String {
    let mount_point = |line: &str| line.split_whitespace().nth(1).map(str::to_string);
    let device_lines: Vec<&str> = device_fstab
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect();
    let mut out = String::new();
    for line in new_fstab.lines() {
        let merged = match mount_point(line) {
            Some(point) if !line.trim_start().starts_with('#') => {
                if point == "/" {
                    let rest: Vec<&str> = line.split_whitespace().skip(1).collect();
                    format!("UUID={root_uuid} {}", rest.join(" "))
                } else {
                    device_lines
                        .iter()
                        .find(|device| mount_point(device).as_deref() == Some(point.as_str()))
                        .map(|device| device.to_string())
                        .unwrap_or_else(|| line.to_string())
                }
            }
            _ => line.to_string(),
        };
        out.push_str(&merged);
        out.push('\n');
    }
    out
}

/// Gives every account of `new_shadow` that the device also has the device's
/// password hash and aging fields; accounts only the new image has keep its
/// entry and accounts only the device had are dropped with it.
/// Whether the carried state's numeric owners mean the same accounts in the
/// new image: root and every account of the project the board has keeps its
/// id there, and no other account of the image takes one of those ids. The
/// state is restored by number while the image allocates its own users, so a
/// different allocation would give one service's files to another (security
/// review of 2026-10-18, N5). Works for `/etc/group` as for `/etc/passwd`.
pub fn owners_agree(device: &str, image: &str) -> std::result::Result<(), String> {
    let entries = |text: &str| -> Vec<(String, String)> {
        text.lines()
            .filter_map(|line| {
                let mut fields = line.split(':');
                let name = fields.next()?.to_string();
                let id = fields.nth(1)?.to_string();
                (!name.is_empty()).then_some((name, id))
            })
            .collect()
    };
    let (device, image) = (entries(device), entries(image));
    for (name, id) in &device {
        if name != "root" && !name.starts_with("hardened-blikvm") {
            continue;
        }
        if let Some((_, other_id)) = image.iter().find(|(other, _)| other == name)
            && other_id != id
        {
            return Err(format!(
                "{name} is {id} on the board and {other_id} in the new image"
            ));
        }
        if let Some((other, _)) = image
            .iter()
            .find(|(other, other_id)| other_id == id && other != name)
        {
            return Err(format!("{name}'s {id} is {other} in the new image"));
        }
    }
    Ok(())
}

pub fn merge_shadow(new_shadow: &str, device_shadow: &str) -> String {
    let name = |line: &str| line.split(':').next().unwrap_or_default().to_string();
    let mut out = String::new();
    for line in new_shadow.lines() {
        let merged = device_shadow
            .lines()
            .find(|device| !line.is_empty() && name(device) == name(line))
            .unwrap_or(line);
        out.push_str(merged);
        out.push('\n');
    }
    out
}

/// Points `rootdev=` of an `armbianEnv.txt` at `root_uuid`.
pub fn patch_armbian_env(text: &str, root_uuid: &str) -> String {
    let mut out = String::new();
    let mut found = false;
    for line in text.lines() {
        if line.starts_with("rootdev=") {
            out.push_str(&format!("rootdev=UUID={root_uuid}\n"));
            found = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !found {
        out.push_str(&format!("rootdev=UUID={root_uuid}\n"));
    }
    out
}

/// The system tools the device uses.
pub struct SystemTools;

fn run(program: &str, args: &[&str]) -> io::Result<()> {
    let status = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{program} failed")))
    }
}

impl Tools for SystemTools {
    fn decompress(
        &mut self,
        payload: &Path,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut child = std::process::Command::new("zstd")
            .args(["-q", "-d", "-c", "--"])
            .arg(payload)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .spawn()?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("no output"))?;
        let mut buffer = vec![0_u8; BUFFER];
        let result = loop {
            match stdout.read(&mut buffer) {
                Ok(0) => break Ok(()),
                Ok(read) => {
                    if let Err(error) = sink(&buffer[..read]) {
                        break Err(error);
                    }
                }
                Err(error) => break Err(error),
            }
        };
        drop(stdout);
        let status = child.wait()?;
        result?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other("zstd could not decompress"))
        }
    }

    fn compress(&mut self, source: &Path, size: u64, target: &Path) -> io::Result<()> {
        let input = File::open(source)?.take(size);
        let output = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(target)?;
        let mut child = std::process::Command::new("zstd")
            .args(["-q", "-1", "-T0", "-c"])
            .stdin(std::process::Stdio::piped())
            .stdout(output)
            .spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("no input"))?;
        let copied = io::copy(&mut io::BufReader::with_capacity(BUFFER, input), &mut stdin);
        drop(stdin);
        let status = child.wait()?;
        copied?;
        if !status.success() {
            return Err(io::Error::other("zstd could not compress"));
        }
        File::open(target)?.sync_all()
    }

    fn finish_filesystem(&mut self, device: &Path, uuid: &str) -> io::Result<()> {
        let device = device
            .to_str()
            .ok_or_else(|| io::Error::other("device path"))?;
        // e2fsck: 0 is clean and 1 means it corrected something; anything
        // else leaves the file system in doubt.
        let status = std::process::Command::new("/sbin/e2fsck")
            .args(["-f", "-p", device])
            .stdin(std::process::Stdio::null())
            .status()?;
        if !matches!(status.code(), Some(0 | 1)) {
            return Err(io::Error::other("e2fsck found errors in the new root"));
        }
        run("/sbin/resize2fs", &[device])?;
        run("/sbin/tune2fs", &["-U", uuid, device])
    }

    fn prepare_root(&mut self, device: &Path, uuid: &str, staging: &Path) -> io::Result<()> {
        let mount = Path::new("/run/hardened-system-update/root");
        fs::create_dir_all(mount)?;
        let device_str = device
            .to_str()
            .ok_or_else(|| io::Error::other("device path"))?;
        let mount_str = mount
            .to_str()
            .ok_or_else(|| io::Error::other("mount path"))?;
        run("/bin/mount", &["-t", "ext4", device_str, mount_str])?;
        let result = (|| -> io::Result<()> {
            // Before anything is changed: the carried owners must mean the
            // same accounts. A stage of an older runtime kept no copies.
            for (kept, image) in [(PASSWD_FILE, "etc/passwd"), (GROUP_FILE, "etc/group")] {
                if let Ok(device) = fs::read_to_string(staging.join(kept)) {
                    owners_agree(&device, &fs::read_to_string(mount.join(image))?).map_err(
                        |conflict| io::Error::other(format!("the accounts differ: {conflict}")),
                    )?;
                }
            }
            let env = mount.join("boot/armbianEnv.txt");
            let text = fs::read_to_string(&env)?;
            fs::write(&env, patch_armbian_env(&text, uuid))?;
            let new_fstab = mount.join("etc/fstab");
            let merged = merge_fstab(
                &fs::read_to_string(&new_fstab)?,
                &fs::read_to_string(staging.join(FSTAB_FILE))?,
                uuid,
            );
            fs::write(&new_fstab, merged)?;
            let new_shadow = mount.join("etc/shadow");
            let merged = merge_shadow(
                &fs::read_to_string(&new_shadow)?,
                &fs::read_to_string(staging.join(SHADOW_FILE))?,
            );
            // Rewrite in place: the file keeps its owner and mode.
            fs::write(&new_shadow, merged)?;
            self.restore_state(mount, &staging.join(STATE_FILE))
        })();
        run("/bin/sync", &[])?;
        run("/bin/umount", &[mount_str])?;
        result
    }

    fn archive_state(&mut self, root: &Path, paths: &[&str], target: &Path) -> io::Result<()> {
        let present: Vec<&str> = paths
            .iter()
            .copied()
            .filter(|path| fs::symlink_metadata(root.join(path)).is_ok())
            .collect();
        let temporary = target.with_extension("tar.new");
        // Not across file systems: the virtual media partition is mounted
        // inside a carried directory, and its images are not device state
        // (security review of 2026-10-18, N3). A file that changes while it
        // is read (tar's status 1, the services run meanwhile) is archived
        // once more; a second change is reported as such.
        for attempt in 0..2 {
            let status = std::process::Command::new("tar")
                .args([
                    "--create",
                    "--one-file-system",
                    "--numeric-owner",
                    "--xattrs",
                    "--acls",
                    "--file",
                ])
                .arg(&temporary)
                .arg("--directory")
                .arg(root)
                .arg("--")
                .args(&present)
                .stdin(std::process::Stdio::null())
                .status()?;
            match status.code() {
                Some(0) => {
                    File::open(&temporary)?.sync_all()?;
                    return fs::rename(temporary, target);
                }
                Some(1) if attempt == 0 => continue,
                Some(1) => {
                    return Err(io::Error::other(
                        "a carried file changed while it was archived, twice",
                    ));
                }
                _ => return Err(io::Error::other("the device state could not be archived")),
            }
        }
        Err(io::Error::other("the device state could not be archived"))
    }

    fn restore_state(&mut self, root: &Path, archive: &Path) -> io::Result<()> {
        let status = std::process::Command::new("tar")
            .args([
                "--extract",
                "--numeric-owner",
                "--preserve-permissions",
                "--same-owner",
                "--xattrs",
                "--acls",
                "--file",
            ])
            .arg(archive)
            .arg("--directory")
            .arg(root)
            .stdin(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other("the device state could not be restored"))
        }
    }
}

/// The trust root file the update-runtime package ships.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustFile {
    pub key_id: String,
    pub public_key: String,
    pub layout_id: String,
}

pub fn load_trust(path: &Path) -> Result<(SystemTrustRoot, String)> {
    let text = fs::read_to_string(path)?;
    let file: TrustFile = serde_json::from_str(&text)
        .map_err(|_| UpdateError::Check("the system trust file is invalid".into()))?;
    let bytes: Vec<u8> = (0..file.public_key.len())
        .step_by(2)
        .filter_map(|index| u8::from_str_radix(file.public_key.get(index..index + 2)?, 16).ok())
        .collect();
    let key: [u8; 32] = bytes
        .try_into()
        .map_err(|_| UpdateError::Check("the system public key is invalid".into()))?;
    let trust = SystemTrustRoot::new(file.key_id, key)
        .map_err(|_| UpdateError::Check("the system trust root is invalid".into()))?;
    Ok((trust, file.layout_id))
}

pub fn staging_path(data_mount: &Path) -> PathBuf {
    data_mount.join(".hardened-system-update")
}
