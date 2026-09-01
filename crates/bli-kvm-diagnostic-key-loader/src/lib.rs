//! Fixed systemd-credential loader for the encrypted diagnostic journal.
//!
//! Production code accepts no caller-selected path. It reads one fixed binary
//! credential from the systemd-provided credential directory for one of two
//! compile-time unit names, validates the directory and file through pinned
//! descriptors, and returns owned typed keys. It has no serializer, key
//! generator, store, collector, acknowledgement, network, logging, HTTP, UI,
//! package, or provisioning surface.

use std::{
    env,
    ffi::CStr,
    fmt,
    fs::{self, File, OpenOptions},
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

use bli_kvm_diagnostic_journal::{
    DIAGNOSTIC_JOURNAL_KEY_BYTES, DiagnosticJournalKey, DiagnosticJournalKeyRing,
};
use nix::unistd::{Group, User, getegid, geteuid};
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

pub const DIAGNOSTIC_KEY_CREDENTIAL_NAME: &str = "diagnostic-journal-keyring";
pub const DIAGNOSTIC_KEY_CREDENTIAL_BYTES: usize = 96;

const CREDENTIALS_DIRECTORY_ENV: &str = "CREDENTIALS_DIRECTORY";
const PLATFORM_BROKER_ACCOUNT: &str = "hardened-blikvm-broker";
const SHARED_GROUP: &str = "hardened-blikvm";
const PLATFORM_BROKER_CREDENTIAL_DIRECTORY: &str =
    "/run/credentials/hardened-blikvm-platform-broker.service";
const MAINTENANCE_CREDENTIAL_DIRECTORY: &str =
    "/run/credentials/hardened-blikvm-diagnostic-maintenance.service";
const CREDENTIAL_MAGIC: &[u8; 8] = b"BLIDKEY\0";
const CREDENTIAL_VERSION: u8 = 1;
const CREDENTIAL_ALGORITHM_XCHACHA20_POLY1305: u8 = 1;
const FLAG_PREVIOUS_KEY: u8 = 1;
const DIRECTORY_MODE_READ_ONLY: u32 = 0o500;
const DIRECTORY_MODE_OWNER_WRITABLE: u32 = 0o700;
const CREDENTIAL_FILE_MODE: u32 = 0o400;

const MAGIC_RANGE: std::ops::Range<usize> = 0..8;
const VERSION_OFFSET: usize = 8;
const ALGORITHM_OFFSET: usize = 9;
const FLAGS_OFFSET: usize = 10;
const RESERVED_BYTE_OFFSET: usize = 11;
const ACTIVE_ID_RANGE: std::ops::Range<usize> = 12..16;
const ACTIVE_KEY_RANGE: std::ops::Range<usize> = 16..48;
const PREVIOUS_ID_RANGE: std::ops::Range<usize> = 48..52;
const PREVIOUS_KEY_RANGE: std::ops::Range<usize> = 52..84;
const RESERVED_RANGE: std::ops::Range<usize> = 84..96;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKeyCredentialError {
    MissingCredentialDirectory,
    InvalidCredentialDirectory,
    IdentityUnavailable,
    WrongProcessIdentity,
    InvalidCredentialFile,
    ConcurrentCredentialChange,
    InvalidCredentialFormat,
}

impl fmt::Display for DiagnosticKeyCredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingCredentialDirectory => "systemd credential directory is unavailable",
            Self::InvalidCredentialDirectory => "systemd credential directory is invalid",
            Self::IdentityUnavailable => "diagnostic credential identity is unavailable",
            Self::WrongProcessIdentity => "diagnostic credential process identity is invalid",
            Self::InvalidCredentialFile => "diagnostic key credential is invalid",
            Self::ConcurrentCredentialChange => "diagnostic key credential changed concurrently",
            Self::InvalidCredentialFormat => "diagnostic key credential format is invalid",
        })
    }
}

impl std::error::Error for DiagnosticKeyCredentialError {}

/// Owned active and optional previous diagnostic journal keys.
///
/// This type intentionally implements neither `Clone` nor `Debug`. The
/// underlying journal key type zeroizes its key bytes on drop.
pub struct LoadedDiagnosticJournalKeys {
    active: DiagnosticJournalKey,
    previous: Option<DiagnosticJournalKey>,
}

impl LoadedDiagnosticJournalKeys {
    pub const fn active_key_id(&self) -> u32 {
        self.active.key_id()
    }

    pub fn previous_key_id(&self) -> Option<u32> {
        self.previous.as_ref().map(DiagnosticJournalKey::key_id)
    }

    pub fn key_ring(&self) -> DiagnosticJournalKeyRing<'_> {
        match DiagnosticJournalKeyRing::new(&self.active, self.previous.as_ref()) {
            Ok(ring) => ring,
            Err(_) => unreachable!("loaded diagnostic key ring was already validated"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ObjectIdentity {
    device: u64,
    inode: u64,
    owner: u32,
    group: u32,
    mode: u32,
    links: u64,
    length: u64,
}

struct CredentialBytes([u8; DIAGNOSTIC_KEY_CREDENTIAL_BYTES]);

impl CredentialBytes {
    const fn zeroed() -> Self {
        Self([0; DIAGNOSTIC_KEY_CREDENTIAL_BYTES])
    }
}

impl Drop for CredentialBytes {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Load the one fixed diagnostic key credential delivered by systemd.
///
/// The environment variable contains only systemd's per-unit directory path;
/// key bytes remain in the credential file. No path argument or alternate
/// credential name is accepted by the production API.
pub fn load_systemd_diagnostic_journal_keys()
-> Result<LoadedDiagnosticJournalKeys, DiagnosticKeyCredentialError> {
    let directory = env::var_os(CREDENTIALS_DIRECTORY_ENV)
        .map(PathBuf::from)
        .ok_or(DiagnosticKeyCredentialError::MissingCredentialDirectory)?;
    if !allowed_systemd_directory(&directory) {
        return Err(DiagnosticKeyCredentialError::InvalidCredentialDirectory);
    }
    let (owner, group) = resolve_runtime_identity()?;
    load_from_directory_with_hook(&directory, owner, group, || {})
}

fn allowed_systemd_directory(directory: &Path) -> bool {
    directory == Path::new(PLATFORM_BROKER_CREDENTIAL_DIRECTORY)
        || directory == Path::new(MAINTENANCE_CREDENTIAL_DIRECTORY)
}

fn resolve_runtime_identity() -> Result<(u32, u32), DiagnosticKeyCredentialError> {
    let user = User::from_name(PLATFORM_BROKER_ACCOUNT)
        .map_err(|_| DiagnosticKeyCredentialError::IdentityUnavailable)?
        .filter(|user| user.name == PLATFORM_BROKER_ACCOUNT && user.uid.as_raw() != 0)
        .ok_or(DiagnosticKeyCredentialError::IdentityUnavailable)?;
    let group = Group::from_name(SHARED_GROUP)
        .map_err(|_| DiagnosticKeyCredentialError::IdentityUnavailable)?
        .filter(|group| group.name == SHARED_GROUP && group.gid.as_raw() != 0)
        .ok_or(DiagnosticKeyCredentialError::IdentityUnavailable)?;
    if geteuid().as_raw() != user.uid.as_raw() || getegid().as_raw() != group.gid.as_raw() {
        return Err(DiagnosticKeyCredentialError::WrongProcessIdentity);
    }
    Ok((user.uid.as_raw(), group.gid.as_raw()))
}

fn load_from_directory_with_hook(
    directory: &Path,
    expected_owner: u32,
    expected_group: u32,
    after_first_read: impl FnOnce(),
) -> Result<LoadedDiagnosticJournalKeys, DiagnosticKeyCredentialError> {
    let (directory_file, directory_identity) =
        open_credential_directory(directory, expected_owner, expected_group)?;
    let (first, first_identity) = read_credential(
        &directory_file,
        directory_identity,
        expected_owner,
        expected_group,
    )?;
    after_first_read();
    let (mut second, second_identity) = read_credential(
        &directory_file,
        directory_identity,
        expected_owner,
        expected_group,
    )?;
    validate_directory_identity(
        directory,
        &directory_file,
        directory_identity,
        expected_owner,
        expected_group,
    )?;
    if first_identity != second_identity || !bool::from(first.0.ct_eq(&second.0)) {
        return Err(DiagnosticKeyCredentialError::ConcurrentCredentialChange);
    }
    decode_credential_in_place(&mut second.0)
}

fn open_credential_directory(
    directory: &Path,
    expected_owner: u32,
    expected_group: u32,
) -> Result<(File, ObjectIdentity), DiagnosticKeyCredentialError> {
    if expected_owner == 0
        || expected_group == 0
        || !directory.is_absolute()
        || directory.file_name().is_none()
    {
        return Err(DiagnosticKeyCredentialError::WrongProcessIdentity);
    }
    let canonical = fs::canonicalize(directory)
        .map_err(|_| DiagnosticKeyCredentialError::InvalidCredentialDirectory)?;
    if canonical != directory {
        return Err(DiagnosticKeyCredentialError::InvalidCredentialDirectory);
    }
    let path_metadata = fs::symlink_metadata(directory)
        .map_err(|_| DiagnosticKeyCredentialError::InvalidCredentialDirectory)?;
    validate_directory_metadata(&path_metadata, expected_owner, expected_group)?;
    let path_identity = object_identity(&path_metadata);
    let directory_file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(directory)
        .map_err(|_| DiagnosticKeyCredentialError::InvalidCredentialDirectory)?;
    let descriptor_metadata = directory_file
        .metadata()
        .map_err(|_| DiagnosticKeyCredentialError::InvalidCredentialDirectory)?;
    validate_directory_metadata(&descriptor_metadata, expected_owner, expected_group)?;
    if object_identity(&descriptor_metadata) != path_identity {
        return Err(DiagnosticKeyCredentialError::InvalidCredentialDirectory);
    }
    validate_no_posix_acl(&directory_file, true)
        .map_err(|_| DiagnosticKeyCredentialError::InvalidCredentialDirectory)?;
    Ok((directory_file, path_identity))
}

fn validate_directory_identity(
    directory: &Path,
    directory_file: &File,
    expected_identity: ObjectIdentity,
    expected_owner: u32,
    expected_group: u32,
) -> Result<(), DiagnosticKeyCredentialError> {
    let canonical = fs::canonicalize(directory)
        .map_err(|_| DiagnosticKeyCredentialError::ConcurrentCredentialChange)?;
    if canonical != directory {
        return Err(DiagnosticKeyCredentialError::ConcurrentCredentialChange);
    }
    let path_metadata = fs::symlink_metadata(directory)
        .map_err(|_| DiagnosticKeyCredentialError::ConcurrentCredentialChange)?;
    validate_directory_metadata(&path_metadata, expected_owner, expected_group)
        .map_err(|_| DiagnosticKeyCredentialError::ConcurrentCredentialChange)?;
    let descriptor_metadata = directory_file
        .metadata()
        .map_err(|_| DiagnosticKeyCredentialError::ConcurrentCredentialChange)?;
    validate_directory_metadata(&descriptor_metadata, expected_owner, expected_group)
        .map_err(|_| DiagnosticKeyCredentialError::ConcurrentCredentialChange)?;
    if object_identity(&path_metadata) != expected_identity
        || object_identity(&descriptor_metadata) != expected_identity
    {
        return Err(DiagnosticKeyCredentialError::ConcurrentCredentialChange);
    }
    validate_no_posix_acl(directory_file, true)
        .map_err(|_| DiagnosticKeyCredentialError::ConcurrentCredentialChange)
}

fn validate_directory_metadata(
    metadata: &fs::Metadata,
    expected_owner: u32,
    expected_group: u32,
) -> Result<(), DiagnosticKeyCredentialError> {
    let mode = metadata.permissions().mode() & 0o7777;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != expected_owner
        || metadata.gid() != expected_group
        || !matches!(
            mode,
            DIRECTORY_MODE_READ_ONLY | DIRECTORY_MODE_OWNER_WRITABLE
        )
    {
        return Err(DiagnosticKeyCredentialError::InvalidCredentialDirectory);
    }
    Ok(())
}

fn read_credential(
    directory: &File,
    directory_identity: ObjectIdentity,
    expected_owner: u32,
    expected_group: u32,
) -> Result<(CredentialBytes, ObjectIdentity), DiagnosticKeyCredentialError> {
    let mut file = open_credential_file(directory)?;
    let before = file
        .metadata()
        .map_err(|_| DiagnosticKeyCredentialError::InvalidCredentialFile)?;
    validate_credential_metadata(&before, directory_identity, expected_owner, expected_group)?;
    validate_no_posix_acl(&file, false)
        .map_err(|_| DiagnosticKeyCredentialError::InvalidCredentialFile)?;
    let before_identity = object_identity(&before);
    let mut bytes = CredentialBytes::zeroed();
    file.read_exact(&mut bytes.0)
        .map_err(|_| DiagnosticKeyCredentialError::InvalidCredentialFile)?;
    let mut extra = [0_u8; 1];
    if file
        .read(&mut extra)
        .map_err(|_| DiagnosticKeyCredentialError::InvalidCredentialFile)?
        != 0
    {
        return Err(DiagnosticKeyCredentialError::InvalidCredentialFile);
    }
    let after = file
        .metadata()
        .map_err(|_| DiagnosticKeyCredentialError::InvalidCredentialFile)?;
    validate_credential_metadata(&after, directory_identity, expected_owner, expected_group)?;
    validate_no_posix_acl(&file, false)
        .map_err(|_| DiagnosticKeyCredentialError::InvalidCredentialFile)?;
    if object_identity(&after) != before_identity {
        return Err(DiagnosticKeyCredentialError::ConcurrentCredentialChange);
    }
    Ok((bytes, before_identity))
}

fn open_credential_file(directory: &File) -> Result<File, DiagnosticKeyCredentialError> {
    let name: &CStr = c"diagnostic-journal-keyring";
    let descriptor = unsafe {
        // SAFETY: the directory descriptor is valid and name is one fixed
        // NUL-terminated component. O_NOFOLLOW prevents symlink traversal and
        // O_NONBLOCK prevents a substituted FIFO from blocking the process.
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if descriptor < 0 {
        return Err(DiagnosticKeyCredentialError::InvalidCredentialFile);
    }
    let owned = unsafe {
        // SAFETY: openat returned a new descriptor transferred exactly once.
        OwnedFd::from_raw_fd(descriptor)
    };
    Ok(File::from(owned))
}

fn validate_credential_metadata(
    metadata: &fs::Metadata,
    directory_identity: ObjectIdentity,
    expected_owner: u32,
    expected_group: u32,
) -> Result<(), DiagnosticKeyCredentialError> {
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.dev() != directory_identity.device
        || metadata.uid() != expected_owner
        || metadata.gid() != expected_group
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o7777 != CREDENTIAL_FILE_MODE
        || metadata.len() != DIAGNOSTIC_KEY_CREDENTIAL_BYTES as u64
    {
        return Err(DiagnosticKeyCredentialError::InvalidCredentialFile);
    }
    Ok(())
}

fn object_identity(metadata: &fs::Metadata) -> ObjectIdentity {
    ObjectIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        owner: metadata.uid(),
        group: metadata.gid(),
        mode: metadata.permissions().mode() & 0o7777,
        links: metadata.nlink(),
        length: metadata.len(),
    }
}

fn validate_no_posix_acl(file: &File, directory: bool) -> Result<(), ()> {
    let names = [c"system.posix_acl_access", c"system.posix_acl_default"];
    let count = if directory { names.len() } else { 1 };
    for name in names.iter().take(count) {
        let result = unsafe {
            // SAFETY: the descriptor and fixed xattr name are valid; null with
            // zero size requests only the attribute length.
            libc::fgetxattr(file.as_raw_fd(), name.as_ptr(), std::ptr::null_mut(), 0)
        };
        if result >= 0 {
            return Err(());
        }
        let code = std::io::Error::last_os_error().raw_os_error();
        if !matches!(code, Some(value) if value == libc::ENODATA || value == libc::ENOTSUP) {
            return Err(());
        }
    }
    Ok(())
}

fn decode_credential_in_place(
    bytes: &mut [u8; DIAGNOSTIC_KEY_CREDENTIAL_BYTES],
) -> Result<LoadedDiagnosticJournalKeys, DiagnosticKeyCredentialError> {
    let result = (|| {
        if &bytes[MAGIC_RANGE] != CREDENTIAL_MAGIC
            || bytes[VERSION_OFFSET] != CREDENTIAL_VERSION
            || bytes[ALGORITHM_OFFSET] != CREDENTIAL_ALGORITHM_XCHACHA20_POLY1305
            || !matches!(bytes[FLAGS_OFFSET], 0 | FLAG_PREVIOUS_KEY)
            || bytes[RESERVED_BYTE_OFFSET] != 0
            || bytes[RESERVED_RANGE].iter().any(|byte| *byte != 0)
        {
            return Err(DiagnosticKeyCredentialError::InvalidCredentialFormat);
        }

        let active_id = u32::from_le_bytes([
            bytes[ACTIVE_ID_RANGE.start],
            bytes[ACTIVE_ID_RANGE.start + 1],
            bytes[ACTIVE_ID_RANGE.start + 2],
            bytes[ACTIVE_ID_RANGE.start + 3],
        ]);
        let mut active_bytes = [0_u8; DIAGNOSTIC_JOURNAL_KEY_BYTES];
        active_bytes.copy_from_slice(&bytes[ACTIVE_KEY_RANGE]);

        let previous_id = u32::from_le_bytes([
            bytes[PREVIOUS_ID_RANGE.start],
            bytes[PREVIOUS_ID_RANGE.start + 1],
            bytes[PREVIOUS_ID_RANGE.start + 2],
            bytes[PREVIOUS_ID_RANGE.start + 3],
        ]);
        let mut previous_bytes = [0_u8; DIAGNOSTIC_JOURNAL_KEY_BYTES];
        previous_bytes.copy_from_slice(&bytes[PREVIOUS_KEY_RANGE]);
        let previous_present = bytes[FLAGS_OFFSET] == FLAG_PREVIOUS_KEY;

        if active_id == 0
            || active_bytes.iter().all(|byte| *byte == 0)
            || (!previous_present
                && (previous_id != 0 || previous_bytes.iter().any(|byte| *byte != 0)))
            || (previous_present
                && (previous_id == 0
                    || previous_id == active_id
                    || previous_bytes.iter().all(|byte| *byte == 0)
                    || bool::from(previous_bytes.ct_eq(&active_bytes))))
        {
            active_bytes.zeroize();
            previous_bytes.zeroize();
            return Err(DiagnosticKeyCredentialError::InvalidCredentialFormat);
        }

        let active = match DiagnosticJournalKey::new(active_id, active_bytes) {
            Ok(active) => active,
            Err(_) => {
                previous_bytes.zeroize();
                return Err(DiagnosticKeyCredentialError::InvalidCredentialFormat);
            }
        };
        let previous = if previous_present {
            Some(
                DiagnosticJournalKey::new(previous_id, previous_bytes)
                    .map_err(|_| DiagnosticKeyCredentialError::InvalidCredentialFormat)?,
            )
        } else {
            previous_bytes.zeroize();
            None
        };
        DiagnosticJournalKeyRing::new(&active, previous.as_ref())
            .map_err(|_| DiagnosticKeyCredentialError::InvalidCredentialFormat)?;
        Ok(LoadedDiagnosticJournalKeys { active, previous })
    })();
    bytes.zeroize();
    result
}

#[cfg(test)]
mod tests;
