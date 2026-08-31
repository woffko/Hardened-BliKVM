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

use bli_kvm_system_action_journal::{
    CopyIndex, JOURNAL_COPY_BYTES, JournalError, RawJournalCopy, SelectedJournal,
    SystemActionPolicyState, plan_initialize, plan_repair, plan_update, select_copies,
    verify_written,
};
pub const SYSTEM_ACTION_STORE_ROOT: &str = "/var/lib/hardened-blikvm-system-actions";
pub const COPY_A_FILE_NAME: &str = "journal-a.bin";
pub const COPY_B_FILE_NAME: &str = "journal-b.bin";

const ROOT_MODE: u32 = 0o700;
const COPY_MODE: u32 = 0o600;
const ROOT_UID: u32 = 0;
const ROOT_GID: u32 = 0;

#[cfg(not(test))]
const LOCK_ACQUIRE_ATTEMPTS: usize = 40;
#[cfg(test)]
const LOCK_ACQUIRE_ATTEMPTS: usize = 2;
#[cfg(not(test))]
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(25);
#[cfg(test)]
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(1);

#[derive(Debug)]
pub enum SystemActionStoreError {
    InvalidRoot,
    UnsafeRoot,
    StoreBusy,
    UnexpectedEntry,
    UnsafeCopy(CopyIndex),
    UnsafeTemporary(CopyIndex),
    AlreadyInitialized,
    ConcurrentStateChange,
    VerificationFailed,
    Journal(JournalError),
    Io(std::io::Error),
    InjectedFault,
}

impl fmt::Display for SystemActionStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRoot => formatter.write_str("system-action store root is invalid"),
            Self::UnsafeRoot => formatter.write_str("system-action store root is unsafe"),
            Self::StoreBusy => formatter.write_str("system-action store is busy"),
            Self::UnexpectedEntry => {
                formatter.write_str("system-action store contains an unexpected entry")
            }
            Self::UnsafeCopy(index) => {
                write!(formatter, "system-action journal copy {index:?} is unsafe")
            }
            Self::UnsafeTemporary(index) => {
                write!(
                    formatter,
                    "system-action temporary copy {index:?} is unsafe"
                )
            }
            Self::AlreadyInitialized => {
                formatter.write_str("system-action store is already initialized")
            }
            Self::ConcurrentStateChange => {
                formatter.write_str("system-action store changed concurrently")
            }
            Self::VerificationFailed => {
                formatter.write_str("system-action journal verification failed")
            }
            Self::Journal(error) => write!(
                formatter,
                "system-action journal model rejected state: {error:?}"
            ),
            Self::Io(_) => formatter.write_str("system-action store I/O failed"),
            Self::InjectedFault => formatter.write_str("injected system-action store fault"),
        }
    }
}

impl std::error::Error for SystemActionStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for SystemActionStoreError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<JournalError> for SystemActionStoreError {
    fn from(value: JournalError) -> Self {
        Self::Journal(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersistedJournal {
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
}

#[derive(Debug)]
pub struct SystemActionStore {
    root: PathBuf,
    root_directory: File,
    identity: RootIdentity,
    expected_owner: u32,
    expected_group: u32,
    operation_lock: Mutex<()>,
    #[cfg(test)]
    test_state: Mutex<TestState>,
}

impl SystemActionStore {
    pub fn open() -> Result<Self, SystemActionStoreError> {
        Self::open_at(PathBuf::from(SYSTEM_ACTION_STORE_ROOT), ROOT_UID, ROOT_GID)
    }

    pub fn load(&self, minimum_generation: u64) -> Result<SelectedJournal, SystemActionStoreError> {
        let _operation = self.lock_operation()?;
        self.prepare_root()?;
        self.load_unlocked(minimum_generation)
    }

    pub fn initialize(
        &self,
        state: SystemActionPolicyState,
    ) -> Result<PersistedJournal, SystemActionStoreError> {
        let _operation = self.lock_operation()?;
        self.begin_write_operation()?;
        let copy_a = self.read_copy(CopyIndex::A)?;
        let copy_b = self.read_copy(CopyIndex::B)?;
        if copy_a.is_some() || copy_b.is_some() {
            return Err(SystemActionStoreError::AlreadyInitialized);
        }
        let plan = plan_initialize(state)?;
        self.replace_copy(plan.first(), plan.bytes())?;
        self.replace_copy(plan.second(), plan.bytes())?;
        let selected = self.load_unlocked(plan.generation())?;
        if selected.degraded()
            || selected.generation() != plan.generation()
            || selected.state() != state
        {
            return Err(SystemActionStoreError::VerificationFailed);
        }
        Ok(PersistedJournal {
            generation: selected.generation(),
        })
    }

    pub fn update(
        &self,
        expected_generation: u64,
        minimum_generation: u64,
        next_state: SystemActionPolicyState,
    ) -> Result<PersistedJournal, SystemActionStoreError> {
        let _operation = self.lock_operation()?;
        self.begin_write_operation()?;
        let selected = self.load_unlocked(minimum_generation)?;
        let plan = plan_update(&selected, expected_generation, next_state)?;
        self.recheck_selected(&selected, minimum_generation)?;
        self.replace_copy(plan.first(), plan.bytes())?;
        self.replace_copy(plan.second(), plan.bytes())?;
        let selected = self.load_unlocked(minimum_generation.max(plan.generation()))?;
        if selected.degraded()
            || selected.generation() != plan.generation()
            || selected.state() != next_state
        {
            return Err(SystemActionStoreError::VerificationFailed);
        }
        Ok(PersistedJournal {
            generation: selected.generation(),
        })
    }

    pub fn repair(
        &self,
        minimum_generation: u64,
    ) -> Result<RepairDisposition, SystemActionStoreError> {
        let _operation = self.lock_operation()?;
        self.begin_write_operation()?;
        let selected = self.load_unlocked(minimum_generation)?;
        let Some(plan) = plan_repair(&selected)? else {
            return Ok(RepairDisposition::NotNeeded);
        };
        self.recheck_selected(&selected, minimum_generation)?;
        self.replace_copy(plan.target(), plan.bytes())?;
        let repaired = self.load_unlocked(minimum_generation.max(plan.generation()))?;
        if repaired.degraded()
            || repaired.generation() != plan.generation()
            || repaired.state() != selected.state()
        {
            return Err(SystemActionStoreError::VerificationFailed);
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
    ) -> Result<Self, SystemActionStoreError> {
        let identity = validate_root_path(&root, expected_owner, expected_group)?;
        let root_directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&root)
            .map_err(|_| SystemActionStoreError::UnsafeRoot)?;
        if identity_from_metadata(&root_directory.metadata()?) != identity {
            return Err(SystemActionStoreError::UnsafeRoot);
        }
        validate_no_posix_acl(&root_directory, true)
            .map_err(|_| SystemActionStoreError::UnsafeRoot)?;
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

    fn lock_operation(&self) -> Result<MutexGuard<'_, ()>, SystemActionStoreError> {
        self.operation_lock
            .lock()
            .map_err(|_| SystemActionStoreError::StoreBusy)
    }

    fn begin_write_operation(&self) -> Result<(), SystemActionStoreError> {
        #[cfg(test)]
        {
            self.test_state
                .lock()
                .map_err(|_| SystemActionStoreError::StoreBusy)?
                .writes = 0;
        }
        self.prepare_root()
    }

    fn prepare_root(&self) -> Result<(), SystemActionStoreError> {
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

    fn validate_root(&self) -> Result<(), SystemActionStoreError> {
        if validate_root_path(&self.root, self.expected_owner, self.expected_group)?
            != self.identity
            || identity_from_metadata(&self.root_directory.metadata()?) != self.identity
        {
            return Err(SystemActionStoreError::UnsafeRoot);
        }
        validate_no_posix_acl(&self.root_directory, true)
            .map_err(|_| SystemActionStoreError::UnsafeRoot)?;
        Ok(())
    }

    fn validate_entries(&self) -> Result<(bool, bool), SystemActionStoreError> {
        let mut temporary_a = false;
        let mut temporary_b = false;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            match name.to_str() {
                Some(name) if name == COPY_A_FILE_NAME || name == COPY_B_FILE_NAME => {}
                Some(".journal-a.tmp") => temporary_a = true,
                Some(".journal-b.tmp") => temporary_b = true,
                _ => return Err(SystemActionStoreError::UnexpectedEntry),
            }
        }
        self.validate_root()?;
        Ok((temporary_a, temporary_b))
    }

    fn load_unlocked(
        &self,
        minimum_generation: u64,
    ) -> Result<SelectedJournal, SystemActionStoreError> {
        self.validate_root()?;
        let copy_a = self.read_copy(CopyIndex::A)?;
        let copy_b = self.read_copy(CopyIndex::B)?;
        select_copies(copy_a.as_deref(), copy_b.as_deref(), minimum_generation).map_err(Into::into)
    }

    fn recheck_selected(
        &self,
        expected: &SelectedJournal,
        minimum_generation: u64,
    ) -> Result<(), SystemActionStoreError> {
        if self.load_unlocked(minimum_generation)? != *expected {
            return Err(SystemActionStoreError::ConcurrentStateChange);
        }
        Ok(())
    }

    fn read_copy(&self, index: CopyIndex) -> Result<Option<Vec<u8>>, SystemActionStoreError> {
        let opened = self
            .open_existing(copy_name(index), false)
            .map_err(|_| SystemActionStoreError::UnsafeCopy(index))?;
        let Some(mut file) = opened else {
            return Ok(None);
        };
        validate_file_metadata(
            &file.metadata()?,
            self.identity,
            self.expected_owner,
            self.expected_group,
            None,
        )
        .map_err(|_| SystemActionStoreError::UnsafeCopy(index))?;
        validate_no_posix_acl(&file, false)
            .map_err(|_| SystemActionStoreError::UnsafeCopy(index))?;
        let length = file.metadata()?.len();
        if length > JOURNAL_COPY_BYTES as u64 {
            return Err(SystemActionStoreError::UnsafeCopy(index));
        }
        let mut bytes = Vec::with_capacity(length as usize);
        Read::by_ref(&mut file)
            .take(JOURNAL_COPY_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| SystemActionStoreError::UnsafeCopy(index))?;
        if bytes.len() as u64 != length {
            return Err(SystemActionStoreError::UnsafeCopy(index));
        }
        Ok(Some(bytes))
    }

    fn replace_copy(
        &self,
        index: CopyIndex,
        bytes: &RawJournalCopy,
    ) -> Result<(), SystemActionStoreError> {
        let _ = self.read_copy(index)?;
        self.cleanup_temporary_if_present(index)?;
        let write_number = self.next_write_number()?;
        let mut temporary = self.create_temporary(index)?;
        temporary.write_all(bytes)?;
        temporary.flush()?;
        self.maybe_fail(write_number, FaultPoint::WriteUnsynced)?;
        temporary.sync_all()?;
        self.maybe_fail(write_number, FaultPoint::TempSynced)?;
        self.verify_open_file(&mut temporary, bytes, index)?;
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
            .ok_or(SystemActionStoreError::VerificationFailed)?;
        verify_written(bytes, &observed).map_err(|_| SystemActionStoreError::VerificationFailed)
    }

    fn create_temporary(&self, index: CopyIndex) -> Result<File, SystemActionStoreError> {
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
            return Err(SystemActionStoreError::Io(std::io::Error::last_os_error()));
        }
        let owned = unsafe {
            // SAFETY: openat returned a new nonnegative descriptor now owned by
            // this function and transferred exactly once into OwnedFd.
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
        .map_err(|_| SystemActionStoreError::UnsafeTemporary(index))?;
        validate_no_posix_acl(&file, false)
            .map_err(|_| SystemActionStoreError::UnsafeTemporary(index))?;
        Ok(file)
    }

    fn verify_open_file(
        &self,
        file: &mut File,
        expected: &RawJournalCopy,
        index: CopyIndex,
    ) -> Result<(), SystemActionStoreError> {
        validate_file_metadata(
            &file.metadata()?,
            self.identity,
            self.expected_owner,
            self.expected_group,
            Some(JOURNAL_COPY_BYTES as u64),
        )
        .map_err(|_| SystemActionStoreError::UnsafeTemporary(index))?;
        validate_no_posix_acl(file, false)
            .map_err(|_| SystemActionStoreError::UnsafeTemporary(index))?;
        file.seek(SeekFrom::Start(0))?;
        let mut observed = [0u8; JOURNAL_COPY_BYTES];
        file.read_exact(&mut observed)?;
        let mut extra = [0u8; 1];
        if file.read(&mut extra)? != 0 {
            return Err(SystemActionStoreError::VerificationFailed);
        }
        verify_written(expected, &observed).map_err(|_| SystemActionStoreError::VerificationFailed)
    }

    fn cleanup_temporary_if_present(&self, index: CopyIndex) -> Result<(), SystemActionStoreError> {
        if self.open_existing(temporary_name(index), true)?.is_some() {
            self.cleanup_temporary(index)?;
            self.root_directory.sync_all()?;
        }
        Ok(())
    }

    fn cleanup_temporary(&self, index: CopyIndex) -> Result<(), SystemActionStoreError> {
        let file = self
            .open_existing(temporary_name(index), true)?
            .ok_or(SystemActionStoreError::UnsafeTemporary(index))?;
        validate_file_metadata(
            &file.metadata()?,
            self.identity,
            self.expected_owner,
            self.expected_group,
            None,
        )
        .map_err(|_| SystemActionStoreError::UnsafeTemporary(index))?;
        validate_no_posix_acl(&file, false)
            .map_err(|_| SystemActionStoreError::UnsafeTemporary(index))?;
        if file.metadata()?.len() > JOURNAL_COPY_BYTES as u64 {
            return Err(SystemActionStoreError::UnsafeTemporary(index));
        }
        unlink_at(&self.root_directory, temporary_name(index))
    }

    fn open_existing(
        &self,
        name: &CStr,
        temporary: bool,
    ) -> Result<Option<File>, SystemActionStoreError> {
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
                Err(SystemActionStoreError::UnsafeTemporary(temporary_index(
                    name,
                )))
            } else {
                Err(SystemActionStoreError::Io(error))
            };
        }
        let owned = unsafe {
            // SAFETY: openat returned a new descriptor transferred once.
            OwnedFd::from_raw_fd(descriptor)
        };
        Ok(Some(File::from(owned)))
    }

    #[cfg(test)]
    fn inject_fault(&self, write_number: usize, point: FaultPoint) {
        self.test_state.lock().unwrap().fault = Some(FaultSpec {
            write_number,
            point,
        });
    }

    fn next_write_number(&self) -> Result<usize, SystemActionStoreError> {
        #[cfg(test)]
        {
            let mut state = self
                .test_state
                .lock()
                .map_err(|_| SystemActionStoreError::StoreBusy)?;
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
    ) -> Result<(), SystemActionStoreError> {
        #[cfg(test)]
        {
            let mut state = self
                .test_state
                .lock()
                .map_err(|_| SystemActionStoreError::StoreBusy)?;
            if state.fault
                == Some(FaultSpec {
                    write_number,
                    point,
                })
            {
                state.fault = None;
                return Err(SystemActionStoreError::InjectedFault);
            }
        }
        #[cfg(not(test))]
        let _ = (write_number, point);
        Ok(())
    }
}

fn validate_root_path(
    path: &Path,
    expected_owner: u32,
    expected_group: u32,
) -> Result<RootIdentity, SystemActionStoreError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(SystemActionStoreError::InvalidRoot);
    }
    let canonical = fs::canonicalize(path).map_err(|_| SystemActionStoreError::UnsafeRoot)?;
    if canonical != path {
        return Err(SystemActionStoreError::UnsafeRoot);
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| SystemActionStoreError::UnsafeRoot)?;
    let identity = identity_from_metadata(&metadata);
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != expected_owner
        || metadata.gid() != expected_group
        || metadata.permissions().mode() & 0o7777 != ROOT_MODE
    {
        return Err(SystemActionStoreError::UnsafeRoot);
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
            // SAFETY: the descriptor and fixed xattr name are valid; a null
            // value with size zero requests only the attribute length.
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

fn acquire_root_lock(root_directory: &File) -> Result<(), SystemActionStoreError> {
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
            return Err(SystemActionStoreError::Io(error));
        }
        if attempt + 1 < LOCK_ACQUIRE_ATTEMPTS {
            thread::sleep(LOCK_RETRY_DELAY);
        }
    }
    Err(SystemActionStoreError::StoreBusy)
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
) -> Result<(), SystemActionStoreError> {
    let result = unsafe {
        // SAFETY: both names are fixed components and both valid descriptors
        // refer to the same locked directory.
        libc::renameat(
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            destination.as_ptr(),
        )
    };
    if result != 0 {
        return Err(SystemActionStoreError::Io(std::io::Error::last_os_error()));
    }
    Ok(())
}

fn unlink_at(directory: &File, name: &CStr) -> Result<(), SystemActionStoreError> {
    let result = unsafe {
        // SAFETY: name is a fixed component and directory is a valid locked FD.
        libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0)
    };
    if result != 0 {
        return Err(SystemActionStoreError::Io(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
