//! Fixed-root atomic storage adapter for the encrypted diagnostic journal.
//!
//! The public store accepts only typed key rings and journal mutations. It has
//! no key-loading, collector, acknowledgement, network, HTTP, UI, logging, or
//! configurable-path surface. The root and every file name are compile-time
//! constants. Tests alone can inject a root, identity, nonce, and durability
//! fault.

use std::{
    ffi::CStr,
    fmt,
    fs::{self, File, OpenOptions, Permissions},
    io::{Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    thread,
    time::Duration,
};

use bli_kvm_diagnostic_journal::{
    CopyIndex, DIAGNOSTIC_JOURNAL_COPY_BYTES, DiagnosticJournal, DiagnosticJournalError,
    DiagnosticJournalKeyRing, DiagnosticJournalNonce, JournalMutation, RawDiagnosticJournalCopy,
    SelectedDiagnosticJournal, plan_initialize, plan_repair, plan_update, select_copies,
    verify_written,
};
use nix::unistd::{Group, User, getegid, geteuid};
#[cfg(not(test))]
use rand_core::{OsRng, RngCore};

pub const DIAGNOSTIC_STORE_ROOT: &str = "/var/lib/hardened-blikvm-diagnostics";
pub const COPY_A_FILE_NAME: &str = "journal-a.bin";
pub const COPY_B_FILE_NAME: &str = "journal-b.bin";

const PLATFORM_BROKER_ACCOUNT: &str = "hardened-blikvm-broker";
const SHARED_GROUP: &str = "hardened-blikvm";
const ROOT_MODE: u32 = 0o700;
const COPY_MODE: u32 = 0o600;

#[cfg(not(test))]
const LOCK_ACQUIRE_ATTEMPTS: usize = 40;
#[cfg(test)]
const LOCK_ACQUIRE_ATTEMPTS: usize = 2;
#[cfg(not(test))]
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(25);
#[cfg(test)]
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticStoreError {
    IdentityUnavailable,
    WrongProcessIdentity,
    InvalidRoot,
    UnsafeRoot,
    StoreBusy,
    UnexpectedEntry,
    UnsafeCopy(CopyIndex),
    UnsafeTemporary(CopyIndex),
    AlreadyInitialized,
    ConcurrentStateChange,
    VerificationFailed,
    NonceUnavailable,
    Journal(DiagnosticJournalError),
    Io,
    InjectedFault,
}

impl fmt::Display for DiagnosticStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::IdentityUnavailable => "diagnostic store identity is unavailable",
            Self::WrongProcessIdentity => "diagnostic store process identity is invalid",
            Self::InvalidRoot => "diagnostic store root is invalid",
            Self::UnsafeRoot => "diagnostic store root is unsafe",
            Self::StoreBusy => "diagnostic store is busy",
            Self::UnexpectedEntry => "diagnostic store contains an unexpected entry",
            Self::UnsafeCopy(_) => "diagnostic journal copy is unsafe",
            Self::UnsafeTemporary(_) => "diagnostic temporary copy is unsafe",
            Self::AlreadyInitialized => "diagnostic store is already initialized",
            Self::ConcurrentStateChange => "diagnostic store changed concurrently",
            Self::VerificationFailed => "diagnostic store verification failed",
            Self::NonceUnavailable => "diagnostic store nonce is unavailable",
            Self::Journal(_) => "diagnostic journal rejected store state",
            Self::Io => "diagnostic store I/O failed",
            Self::InjectedFault => "diagnostic store injected fault",
        })
    }
}

impl std::error::Error for DiagnosticStoreError {}

impl From<std::io::Error> for DiagnosticStoreError {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}

impl From<DiagnosticJournalError> for DiagnosticStoreError {
    fn from(error: DiagnosticJournalError) -> Self {
        Self::Journal(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistedDiagnosticJournal {
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairDisposition {
    NotNeeded,
    Repaired { generation: u64, copy: CopyIndex },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RootIdentity {
    device: u64,
    inode: u64,
    owner: u32,
    group: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FaultPoint {
    WriteUnsynced,
    TempSynced,
    RenamedUnsyncedDirectory,
    DirectorySyncedUnverified,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FaultSpec {
    write_number: usize,
    point: FaultPoint,
}

#[cfg(test)]
#[derive(Debug, Default)]
struct TestState {
    writes: usize,
    fault: Option<FaultSpec>,
    nonces: std::collections::VecDeque<[u8; 24]>,
}

/// Single-process owner for one locked diagnostic store.
///
/// It intentionally implements neither `Clone` nor `Debug`.
pub struct DiagnosticJournalStore {
    root: PathBuf,
    root_directory: File,
    identity: RootIdentity,
    expected_owner: u32,
    expected_group: u32,
    operation_lock: Mutex<()>,
    #[cfg(test)]
    test_state: Mutex<TestState>,
}

impl DiagnosticJournalStore {
    pub fn open() -> Result<Self, DiagnosticStoreError> {
        let (owner, group) = resolve_runtime_identity()?;
        Self::open_at(PathBuf::from(DIAGNOSTIC_STORE_ROOT), owner, group)
    }

    pub fn load(
        &self,
        keys: &DiagnosticJournalKeyRing<'_>,
        minimum_generation: u64,
    ) -> Result<SelectedDiagnosticJournal, DiagnosticStoreError> {
        let _operation = self.lock_operation()?;
        self.prepare_root()?;
        self.load_unlocked(keys, minimum_generation)
    }

    pub fn initialize(
        &self,
        keys: &DiagnosticJournalKeyRing<'_>,
    ) -> Result<PersistedDiagnosticJournal, DiagnosticStoreError> {
        let _operation = self.lock_operation()?;
        self.begin_write_operation()?;
        let copy_a = self.read_copy(CopyIndex::A)?;
        let copy_b = self.read_copy(CopyIndex::B)?;
        if copy_a.is_some() || copy_b.is_some() {
            return Err(DiagnosticStoreError::AlreadyInitialized);
        }
        let nonce = self.fresh_nonce()?;
        let empty = DiagnosticJournal::empty();
        let plan = plan_initialize(&empty, keys, nonce)?;
        self.replace_copy(plan.first(), plan.bytes(), keys)?;
        self.replace_copy(plan.second(), plan.bytes(), keys)?;
        let selected = self.load_unlocked(keys, plan.generation())?;
        if selected.degraded()
            || selected.generation() != plan.generation()
            || selected.journal().entry_count() != 0
        {
            return Err(DiagnosticStoreError::VerificationFailed);
        }
        Ok(PersistedDiagnosticJournal {
            generation: selected.generation(),
        })
    }

    pub fn update(
        &self,
        keys: &DiagnosticJournalKeyRing<'_>,
        minimum_generation: u64,
        mutation: &JournalMutation,
    ) -> Result<PersistedDiagnosticJournal, DiagnosticStoreError> {
        let _operation = self.lock_operation()?;
        self.begin_write_operation()?;
        let selected = self.load_unlocked(keys, minimum_generation)?;
        let nonce = self.fresh_nonce()?;
        let plan = plan_update(&selected, mutation, keys, nonce)?;
        self.recheck_selected(&selected, keys, minimum_generation)?;
        self.replace_copy(plan.first(), plan.bytes(), keys)?;
        self.replace_copy(plan.second(), plan.bytes(), keys)?;
        let selected = self.load_unlocked(keys, minimum_generation.max(plan.generation()))?;
        if selected.degraded()
            || selected.generation() != plan.generation()
            || selected.journal() != mutation.journal()
        {
            return Err(DiagnosticStoreError::VerificationFailed);
        }
        Ok(PersistedDiagnosticJournal {
            generation: selected.generation(),
        })
    }

    pub fn repair(
        &self,
        keys: &DiagnosticJournalKeyRing<'_>,
        minimum_generation: u64,
    ) -> Result<RepairDisposition, DiagnosticStoreError> {
        let _operation = self.lock_operation()?;
        self.begin_write_operation()?;
        let selected = self.load_unlocked(keys, minimum_generation)?;
        let Some(plan) = plan_repair(&selected) else {
            return Ok(RepairDisposition::NotNeeded);
        };
        self.recheck_selected(&selected, keys, minimum_generation)?;
        self.replace_copy(plan.target(), plan.bytes(), keys)?;
        let repaired =
            self.load_unlocked(keys, minimum_generation.max(plan.expected_generation()))?;
        if repaired.degraded()
            || repaired.generation() != plan.expected_generation()
            || repaired.journal() != selected.journal()
        {
            return Err(DiagnosticStoreError::VerificationFailed);
        }
        Ok(RepairDisposition::Repaired {
            generation: repaired.generation(),
            copy: plan.target(),
        })
    }

    fn open_at(
        root: PathBuf,
        expected_owner: u32,
        expected_group: u32,
    ) -> Result<Self, DiagnosticStoreError> {
        if expected_owner == 0 || expected_group == 0 {
            return Err(DiagnosticStoreError::WrongProcessIdentity);
        }
        let identity = validate_root_path(&root, expected_owner, expected_group)?;
        let root_directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&root)
            .map_err(|_| DiagnosticStoreError::UnsafeRoot)?;
        if identity_from_metadata(&root_directory.metadata()?) != identity {
            return Err(DiagnosticStoreError::UnsafeRoot);
        }
        validate_no_posix_acl(&root_directory, true)
            .map_err(|_| DiagnosticStoreError::UnsafeRoot)?;
        acquire_root_lock(&root_directory)?;
        let store = Self {
            root,
            root_directory,
            identity,
            expected_owner,
            expected_group,
            operation_lock: Mutex::new(()),
            #[cfg(test)]
            test_state: Mutex::new(TestState::default()),
        };
        store.prepare_root()?;
        let _ = store.read_copy(CopyIndex::A)?;
        let _ = store.read_copy(CopyIndex::B)?;
        Ok(store)
    }

    fn lock_operation(&self) -> Result<MutexGuard<'_, ()>, DiagnosticStoreError> {
        self.operation_lock
            .lock()
            .map_err(|_| DiagnosticStoreError::StoreBusy)
    }

    fn begin_write_operation(&self) -> Result<(), DiagnosticStoreError> {
        #[cfg(test)]
        {
            self.test_state
                .lock()
                .map_err(|_| DiagnosticStoreError::StoreBusy)?
                .writes = 0;
        }
        self.prepare_root()
    }

    fn prepare_root(&self) -> Result<(), DiagnosticStoreError> {
        self.validate_root()?;
        let (temporary_a, temporary_b) = self.validate_entries()?;
        let mut removed = false;
        if temporary_a {
            self.cleanup_temporary(CopyIndex::A)?;
            removed = true;
        }
        if temporary_b {
            self.cleanup_temporary(CopyIndex::B)?;
            removed = true;
        }
        if removed {
            self.root_directory.sync_all()?;
        }
        self.validate_root()
    }

    fn validate_root(&self) -> Result<(), DiagnosticStoreError> {
        if validate_root_path(&self.root, self.expected_owner, self.expected_group)?
            != self.identity
            || identity_from_metadata(&self.root_directory.metadata()?) != self.identity
        {
            return Err(DiagnosticStoreError::UnsafeRoot);
        }
        validate_no_posix_acl(&self.root_directory, true)
            .map_err(|_| DiagnosticStoreError::UnsafeRoot)?;
        Ok(())
    }

    fn validate_entries(&self) -> Result<(bool, bool), DiagnosticStoreError> {
        let mut temporary_a = false;
        let mut temporary_b = false;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            match name.to_str() {
                Some(name) if name == COPY_A_FILE_NAME || name == COPY_B_FILE_NAME => {}
                Some(".journal-a.tmp") => temporary_a = true,
                Some(".journal-b.tmp") => temporary_b = true,
                _ => return Err(DiagnosticStoreError::UnexpectedEntry),
            }
        }
        self.validate_root()?;
        Ok((temporary_a, temporary_b))
    }

    fn load_unlocked(
        &self,
        keys: &DiagnosticJournalKeyRing<'_>,
        minimum_generation: u64,
    ) -> Result<SelectedDiagnosticJournal, DiagnosticStoreError> {
        self.validate_root()?;
        let copy_a = self.read_copy(CopyIndex::A)?;
        let copy_b = self.read_copy(CopyIndex::B)?;
        self.validate_root()?;
        select_copies(
            copy_a.as_deref(),
            copy_b.as_deref(),
            keys,
            minimum_generation,
        )
        .map_err(Into::into)
    }

    fn recheck_selected(
        &self,
        expected: &SelectedDiagnosticJournal,
        keys: &DiagnosticJournalKeyRing<'_>,
        minimum_generation: u64,
    ) -> Result<(), DiagnosticStoreError> {
        let observed = self.load_unlocked(keys, minimum_generation)?;
        if observed.selected_index() != expected.selected_index()
            || observed.generation() != expected.generation()
            || observed.degraded() != expected.degraded()
            || observed.repair_target() != expected.repair_target()
            || observed.journal() != expected.journal()
            || observed.selected_bytes() != expected.selected_bytes()
        {
            return Err(DiagnosticStoreError::ConcurrentStateChange);
        }
        Ok(())
    }

    fn read_copy(&self, index: CopyIndex) -> Result<Option<Vec<u8>>, DiagnosticStoreError> {
        let opened = self
            .open_existing(copy_name(index), false)
            .map_err(|_| DiagnosticStoreError::UnsafeCopy(index))?;
        let Some(mut file) = opened else {
            return Ok(None);
        };
        let metadata = file.metadata()?;
        validate_file_metadata(
            &metadata,
            self.identity,
            self.expected_owner,
            self.expected_group,
            None,
        )
        .map_err(|_| DiagnosticStoreError::UnsafeCopy(index))?;
        validate_no_posix_acl(&file, false).map_err(|_| DiagnosticStoreError::UnsafeCopy(index))?;
        let length = metadata.len();
        if length > DIAGNOSTIC_JOURNAL_COPY_BYTES as u64 {
            return Err(DiagnosticStoreError::UnsafeCopy(index));
        }
        let mut bytes = Vec::with_capacity(length as usize);
        Read::by_ref(&mut file)
            .take(DIAGNOSTIC_JOURNAL_COPY_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| DiagnosticStoreError::UnsafeCopy(index))?;
        if bytes.len() as u64 != length {
            return Err(DiagnosticStoreError::UnsafeCopy(index));
        }
        validate_file_metadata(
            &file.metadata()?,
            self.identity,
            self.expected_owner,
            self.expected_group,
            Some(length),
        )
        .map_err(|_| DiagnosticStoreError::UnsafeCopy(index))?;
        validate_no_posix_acl(&file, false).map_err(|_| DiagnosticStoreError::UnsafeCopy(index))?;
        Ok(Some(bytes))
    }

    fn replace_copy(
        &self,
        index: CopyIndex,
        bytes: &RawDiagnosticJournalCopy,
        keys: &DiagnosticJournalKeyRing<'_>,
    ) -> Result<(), DiagnosticStoreError> {
        let _ = self.read_copy(index)?;
        self.cleanup_temporary_if_present(index)?;
        let write_number = self.next_write_number()?;
        let mut temporary = self.create_temporary(index)?;
        temporary.write_all(bytes)?;
        temporary.flush()?;
        self.maybe_fail(write_number, FaultPoint::WriteUnsynced)?;
        temporary.sync_all()?;
        self.maybe_fail(write_number, FaultPoint::TempSynced)?;
        self.verify_open_file(&mut temporary, bytes, index, keys)?;
        rename_at(
            &self.root_directory,
            temporary_name(index),
            copy_name(index),
        )?;
        self.maybe_fail(write_number, FaultPoint::RenamedUnsyncedDirectory)?;
        self.root_directory.sync_all()?;
        self.maybe_fail(write_number, FaultPoint::DirectorySyncedUnverified)?;
        let observed = self
            .read_copy(index)?
            .ok_or(DiagnosticStoreError::VerificationFailed)?;
        self.validate_root()?;
        verify_written(bytes, &observed, keys).map_err(|_| DiagnosticStoreError::VerificationFailed)
    }

    fn create_temporary(&self, index: CopyIndex) -> Result<File, DiagnosticStoreError> {
        let descriptor = unsafe {
            // SAFETY: the directory descriptor is owned and valid, the name is
            // a fixed NUL-terminated component, and the mode argument is valid.
            libc::openat(
                self.root_directory.as_raw_fd(),
                temporary_name(index).as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                COPY_MODE as libc::mode_t,
            )
        };
        if descriptor < 0 {
            return Err(DiagnosticStoreError::Io);
        }
        let owned = unsafe {
            // SAFETY: openat returned a new nonnegative descriptor transferred
            // exactly once into OwnedFd.
            OwnedFd::from_raw_fd(descriptor)
        };
        let file = File::from(owned);
        file.set_permissions(Permissions::from_mode(COPY_MODE))?;
        validate_file_metadata(
            &file.metadata()?,
            self.identity,
            self.expected_owner,
            self.expected_group,
            Some(0),
        )
        .map_err(|_| DiagnosticStoreError::UnsafeTemporary(index))?;
        validate_no_posix_acl(&file, false)
            .map_err(|_| DiagnosticStoreError::UnsafeTemporary(index))?;
        Ok(file)
    }

    fn verify_open_file(
        &self,
        file: &mut File,
        expected: &RawDiagnosticJournalCopy,
        index: CopyIndex,
        keys: &DiagnosticJournalKeyRing<'_>,
    ) -> Result<(), DiagnosticStoreError> {
        validate_file_metadata(
            &file.metadata()?,
            self.identity,
            self.expected_owner,
            self.expected_group,
            Some(DIAGNOSTIC_JOURNAL_COPY_BYTES as u64),
        )
        .map_err(|_| DiagnosticStoreError::UnsafeTemporary(index))?;
        validate_no_posix_acl(file, false)
            .map_err(|_| DiagnosticStoreError::UnsafeTemporary(index))?;
        file.seek(SeekFrom::Start(0))?;
        let mut observed = [0_u8; DIAGNOSTIC_JOURNAL_COPY_BYTES];
        file.read_exact(&mut observed)?;
        let mut extra = [0_u8; 1];
        if file.read(&mut extra)? != 0 {
            return Err(DiagnosticStoreError::VerificationFailed);
        }
        validate_file_metadata(
            &file.metadata()?,
            self.identity,
            self.expected_owner,
            self.expected_group,
            Some(DIAGNOSTIC_JOURNAL_COPY_BYTES as u64),
        )
        .map_err(|_| DiagnosticStoreError::UnsafeTemporary(index))?;
        validate_no_posix_acl(file, false)
            .map_err(|_| DiagnosticStoreError::UnsafeTemporary(index))?;
        verify_written(expected, &observed, keys)
            .map_err(|_| DiagnosticStoreError::VerificationFailed)
    }

    fn cleanup_temporary_if_present(&self, index: CopyIndex) -> Result<(), DiagnosticStoreError> {
        if self.open_existing(temporary_name(index), true)?.is_some() {
            self.cleanup_temporary(index)?;
            self.root_directory.sync_all()?;
        }
        Ok(())
    }

    fn cleanup_temporary(&self, index: CopyIndex) -> Result<(), DiagnosticStoreError> {
        let file = self
            .open_existing(temporary_name(index), true)?
            .ok_or(DiagnosticStoreError::UnsafeTemporary(index))?;
        validate_file_metadata(
            &file.metadata()?,
            self.identity,
            self.expected_owner,
            self.expected_group,
            None,
        )
        .map_err(|_| DiagnosticStoreError::UnsafeTemporary(index))?;
        validate_no_posix_acl(&file, false)
            .map_err(|_| DiagnosticStoreError::UnsafeTemporary(index))?;
        if file.metadata()?.len() > DIAGNOSTIC_JOURNAL_COPY_BYTES as u64 {
            return Err(DiagnosticStoreError::UnsafeTemporary(index));
        }
        unlink_at(&self.root_directory, temporary_name(index))
    }

    fn open_existing(
        &self,
        name: &CStr,
        temporary: bool,
    ) -> Result<Option<File>, DiagnosticStoreError> {
        let descriptor = unsafe {
            // SAFETY: the directory descriptor is valid and name is one fixed
            // NUL-terminated component. O_NOFOLLOW prevents symlink traversal.
            libc::openat(
                self.root_directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if descriptor < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return if temporary {
                Err(DiagnosticStoreError::UnsafeTemporary(temporary_index(name)))
            } else {
                Err(DiagnosticStoreError::Io)
            };
        }
        let owned = unsafe {
            // SAFETY: openat returned a new descriptor transferred once.
            OwnedFd::from_raw_fd(descriptor)
        };
        Ok(Some(File::from(owned)))
    }

    fn fresh_nonce(&self) -> Result<DiagnosticJournalNonce, DiagnosticStoreError> {
        #[cfg(test)]
        {
            let bytes = self
                .test_state
                .lock()
                .map_err(|_| DiagnosticStoreError::StoreBusy)?
                .nonces
                .pop_front()
                .ok_or(DiagnosticStoreError::NonceUnavailable)?;
            DiagnosticJournalNonce::new(bytes).ok_or(DiagnosticStoreError::NonceUnavailable)
        }
        #[cfg(not(test))]
        {
            let mut bytes = [0_u8; 24];
            OsRng
                .try_fill_bytes(&mut bytes)
                .map_err(|_| DiagnosticStoreError::NonceUnavailable)?;
            DiagnosticJournalNonce::new(bytes).ok_or(DiagnosticStoreError::NonceUnavailable)
        }
    }

    #[cfg(test)]
    fn inject_nonce(&self, bytes: [u8; 24]) {
        self.test_state.lock().unwrap().nonces.push_back(bytes);
    }

    #[cfg(test)]
    fn inject_fault(&self, write_number: usize, point: FaultPoint) {
        self.test_state.lock().unwrap().fault = Some(FaultSpec {
            write_number,
            point,
        });
    }

    fn next_write_number(&self) -> Result<usize, DiagnosticStoreError> {
        #[cfg(test)]
        {
            let mut state = self
                .test_state
                .lock()
                .map_err(|_| DiagnosticStoreError::StoreBusy)?;
            state.writes = state.writes.saturating_add(1);
            Ok(state.writes)
        }
        #[cfg(not(test))]
        {
            Ok(0)
        }
    }

    fn maybe_fail(
        &self,
        write_number: usize,
        point: FaultPoint,
    ) -> Result<(), DiagnosticStoreError> {
        #[cfg(test)]
        {
            let mut state = self
                .test_state
                .lock()
                .map_err(|_| DiagnosticStoreError::StoreBusy)?;
            if state.fault
                == Some(FaultSpec {
                    write_number,
                    point,
                })
            {
                state.fault = None;
                return Err(DiagnosticStoreError::InjectedFault);
            }
        }
        #[cfg(not(test))]
        let _ = (write_number, point);
        Ok(())
    }
}

fn resolve_runtime_identity() -> Result<(u32, u32), DiagnosticStoreError> {
    let user = User::from_name(PLATFORM_BROKER_ACCOUNT)
        .map_err(|_| DiagnosticStoreError::IdentityUnavailable)?
        .filter(|user| user.name == PLATFORM_BROKER_ACCOUNT && user.uid.as_raw() != 0)
        .ok_or(DiagnosticStoreError::IdentityUnavailable)?;
    let group = Group::from_name(SHARED_GROUP)
        .map_err(|_| DiagnosticStoreError::IdentityUnavailable)?
        .filter(|group| group.name == SHARED_GROUP && group.gid.as_raw() != 0)
        .ok_or(DiagnosticStoreError::IdentityUnavailable)?;
    if geteuid().as_raw() != user.uid.as_raw() || getegid().as_raw() != group.gid.as_raw() {
        return Err(DiagnosticStoreError::WrongProcessIdentity);
    }
    Ok((user.uid.as_raw(), group.gid.as_raw()))
}

fn validate_root_path(
    path: &Path,
    expected_owner: u32,
    expected_group: u32,
) -> Result<RootIdentity, DiagnosticStoreError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(DiagnosticStoreError::InvalidRoot);
    }
    let canonical = fs::canonicalize(path).map_err(|_| DiagnosticStoreError::UnsafeRoot)?;
    if canonical != path {
        return Err(DiagnosticStoreError::UnsafeRoot);
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| DiagnosticStoreError::UnsafeRoot)?;
    let identity = identity_from_metadata(&metadata);
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != expected_owner
        || metadata.gid() != expected_group
        || metadata.permissions().mode() & 0o7777 != ROOT_MODE
    {
        return Err(DiagnosticStoreError::UnsafeRoot);
    }
    Ok(identity)
}

fn validate_file_metadata(
    metadata: &fs::Metadata,
    root: RootIdentity,
    expected_owner: u32,
    expected_group: u32,
    expected_length: Option<u64>,
) -> Result<(), ()> {
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.dev() != root.device
        || metadata.uid() != expected_owner
        || metadata.gid() != expected_group
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o7777 != COPY_MODE
        || expected_length.is_some_and(|length| metadata.len() != length)
    {
        return Err(());
    }
    Ok(())
}

fn identity_from_metadata(metadata: &fs::Metadata) -> RootIdentity {
    RootIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        owner: metadata.uid(),
        group: metadata.gid(),
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

fn acquire_root_lock(root_directory: &File) -> Result<(), DiagnosticStoreError> {
    for attempt in 0..LOCK_ACQUIRE_ATTEMPTS {
        let result = unsafe {
            // SAFETY: flock observes only the valid owned directory descriptor.
            libc::flock(root_directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)
        };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        let contended = matches!(
            error.raw_os_error(),
            Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK
        );
        if !contended {
            return Err(DiagnosticStoreError::Io);
        }
        if attempt + 1 < LOCK_ACQUIRE_ATTEMPTS {
            thread::sleep(LOCK_RETRY_DELAY);
        }
    }
    Err(DiagnosticStoreError::StoreBusy)
}

fn copy_name(index: CopyIndex) -> &'static CStr {
    match index {
        CopyIndex::A => c"journal-a.bin",
        CopyIndex::B => c"journal-b.bin",
    }
}

fn temporary_name(index: CopyIndex) -> &'static CStr {
    match index {
        CopyIndex::A => c".journal-a.tmp",
        CopyIndex::B => c".journal-b.tmp",
    }
}

fn temporary_index(name: &CStr) -> CopyIndex {
    if name == c".journal-a.tmp" {
        CopyIndex::A
    } else {
        CopyIndex::B
    }
}

fn rename_at(
    directory: &File,
    source: &CStr,
    destination: &CStr,
) -> Result<(), DiagnosticStoreError> {
    let result = unsafe {
        // SAFETY: both names are fixed components and the valid descriptor
        // refers to the same locked directory.
        libc::renameat(
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            destination.as_ptr(),
        )
    };
    if result != 0 {
        return Err(DiagnosticStoreError::Io);
    }
    Ok(())
}

fn unlink_at(directory: &File, name: &CStr) -> Result<(), DiagnosticStoreError> {
    let result = unsafe {
        // SAFETY: name is a fixed component and directory is a valid locked FD.
        libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0)
    };
    if result != 0 {
        return Err(DiagnosticStoreError::Io);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
