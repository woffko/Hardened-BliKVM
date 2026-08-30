use std::{
    fs::{self, File, OpenOptions, Permissions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use crate::{UpdateManagerError, model::JournalDocument};

pub(crate) const JOURNAL_FILE_NAME: &str = "application-state.json";
const MAX_JOURNAL_BYTES: u64 = 64 * 1024;
const LOCK_ACQUIRE_ATTEMPTS: usize = 40;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectoryIdentity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
    pub(crate) owner: u32,
}

#[derive(Debug)]
pub(crate) struct StateStore {
    root: PathBuf,
    path: PathBuf,
    root_directory: File,
    identity: DirectoryIdentity,
}

impl StateStore {
    pub(crate) fn open(root: PathBuf) -> Result<Self, UpdateManagerError> {
        let identity = validate_directory(&root)?;
        let root_directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&root)
            .map_err(|_| UpdateManagerError::UnsafeStateRoot)?;
        if identity_from_metadata(&root_directory.metadata()?) != identity {
            return Err(UpdateManagerError::UnsafeStateRoot);
        }
        acquire_state_root_lock(&root_directory)?;
        let path = root.join(JOURNAL_FILE_NAME);
        let store = Self {
            root,
            path,
            root_directory,
            identity,
        };
        store.validate_root()?;
        let _ = store.load()?;
        Ok(store)
    }

    pub(crate) fn root_path(&self) -> &Path {
        &self.root
    }

    pub(crate) fn device(&self) -> u64 {
        self.identity.device
    }

    pub(crate) fn load(&self) -> Result<Option<JournalDocument>, UpdateManagerError> {
        self.validate_root()?;
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(UpdateManagerError::Io(error)),
        };
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.dev() != self.identity.device
            || metadata.uid() != self.identity.owner
            || metadata.nlink() != 1
            || metadata.permissions().mode() & 0o7777 != 0o600
            || metadata.len() == 0
            || metadata.len() > MAX_JOURNAL_BYTES
        {
            return Err(UpdateManagerError::UnsafeJournal);
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&self.path)
            .map_err(|_| UpdateManagerError::UnsafeJournal)?;
        let opened_metadata = file
            .metadata()
            .map_err(|_| UpdateManagerError::UnsafeJournal)?;
        if opened_metadata.dev() != metadata.dev()
            || opened_metadata.ino() != metadata.ino()
            || opened_metadata.len() != metadata.len()
        {
            return Err(UpdateManagerError::UnsafeJournal);
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_JOURNAL_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != metadata.len() {
            return Err(UpdateManagerError::UnsafeJournal);
        }
        let document: JournalDocument =
            serde_json::from_slice(&bytes).map_err(|_| UpdateManagerError::InvalidJournal)?;
        document.validate()?;
        Ok(Some(document))
    }

    pub(crate) fn store(
        &self,
        document: &JournalDocument,
        expected_generation: Option<u64>,
    ) -> Result<(), UpdateManagerError> {
        self.validate_root()?;
        document.validate()?;
        match (expected_generation, self.load()?) {
            (None, None) => {}
            (Some(expected), Some(current)) if current.generation == expected => {}
            _ => return Err(UpdateManagerError::ConcurrentStateChange),
        }
        let bytes = serde_json::to_vec(document).map_err(|_| UpdateManagerError::InvalidJournal)?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(UpdateManagerError::JournalTooLarge);
        }
        let mut temporary = tempfile::NamedTempFile::new_in(&self.root)?;
        fs::set_permissions(temporary.path(), Permissions::from_mode(0o600))?;
        let temporary_metadata = temporary.as_file().metadata()?;
        if !temporary_metadata.is_file()
            || temporary_metadata.dev() != self.identity.device
            || temporary_metadata.uid() != self.identity.owner
            || temporary_metadata.nlink() != 1
            || temporary_metadata.permissions().mode() & 0o7777 != 0o600
        {
            return Err(UpdateManagerError::UnsafeStateRoot);
        }
        temporary.write_all(&bytes)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&self.path)
            .map_err(|error| UpdateManagerError::Io(error.error))?;
        self.root_directory.sync_all()?;
        if self.load()?.as_ref() != Some(document) {
            return Err(UpdateManagerError::InvalidJournal);
        }
        Ok(())
    }

    fn validate_root(&self) -> Result<(), UpdateManagerError> {
        if validate_directory(&self.root)? != self.identity
            || identity_from_metadata(&self.root_directory.metadata()?) != self.identity
        {
            return Err(UpdateManagerError::UnsafeStateRoot);
        }
        Ok(())
    }
}

fn acquire_state_root_lock(root_directory: &File) -> Result<(), UpdateManagerError> {
    for attempt in 0..LOCK_ACQUIRE_ATTEMPTS {
        // SAFETY: flock observes only the valid owned directory descriptor.
        let result =
            unsafe { libc::flock(root_directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(());
        }

        let error = std::io::Error::last_os_error();
        let contended = matches!(
            error.raw_os_error(),
            Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK
        );
        if !contended {
            return Err(UpdateManagerError::Io(error));
        }
        if attempt + 1 < LOCK_ACQUIRE_ATTEMPTS {
            thread::sleep(LOCK_RETRY_DELAY);
        }
    }
    Err(UpdateManagerError::ManagerBusy)
}

fn validate_directory(path: &Path) -> Result<DirectoryIdentity, UpdateManagerError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(UpdateManagerError::InvalidStateRoot);
    }
    let canonical = fs::canonicalize(path).map_err(|_| UpdateManagerError::UnsafeStateRoot)?;
    if canonical != path {
        return Err(UpdateManagerError::UnsafeStateRoot);
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| UpdateManagerError::UnsafeStateRoot)?;
    let identity = identity_from_metadata(&metadata);
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o022 != 0
        || identity.owner != effective_uid()
    {
        return Err(UpdateManagerError::UnsafeStateRoot);
    }
    Ok(identity)
}

fn identity_from_metadata(metadata: &fs::Metadata) -> DirectoryIdentity {
    DirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        owner: metadata.uid(),
    }
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }
}
