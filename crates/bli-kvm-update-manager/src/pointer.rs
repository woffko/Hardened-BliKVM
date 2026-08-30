use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt, symlink},
    },
    path::{Component, Path, PathBuf},
};

use crate::{UpdateManagerError, model::valid_slot_id};

pub(crate) const CURRENT_LINK_NAME: &str = "current";
const TEMPORARY_LINK_NAME: &str = ".current-next";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PointerIdentity {
    device: u64,
    inode: u64,
    owner: u32,
}

#[derive(Debug)]
pub(crate) struct PointerStore {
    root: PathBuf,
    current: PathBuf,
    temporary: PathBuf,
    root_directory: File,
    identity: PointerIdentity,
}

impl PointerStore {
    pub(crate) fn open(root: PathBuf) -> Result<Self, UpdateManagerError> {
        let identity = validate_directory(&root)?;
        let root_directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&root)
            .map_err(|_| UpdateManagerError::UnsafeSlotRoot)?;
        if identity_from_metadata(&root_directory.metadata()?) != identity {
            return Err(UpdateManagerError::UnsafeSlotRoot);
        }
        let store = Self {
            current: root.join(CURRENT_LINK_NAME),
            temporary: root.join(TEMPORARY_LINK_NAME),
            root,
            root_directory,
            identity,
        };
        store.validate_root()?;
        store.validate_current_node_if_present()?;
        store.validate_temporary_if_present()?;
        Ok(store)
    }

    pub(crate) fn root_path(&self) -> &Path {
        &self.root
    }

    pub(crate) fn device(&self) -> u64 {
        self.identity.device
    }

    pub(crate) fn read(&self) -> Result<Option<String>, UpdateManagerError> {
        self.validate_root()?;
        let metadata = match fs::symlink_metadata(&self.current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(UpdateManagerError::Io(error)),
        };
        validate_owned_symlink(&metadata, self.identity)?;
        let target = fs::read_link(&self.current).map_err(|_| UpdateManagerError::UnsafePointer)?;
        let mut components = target.components();
        let Some(Component::Normal(component)) = components.next() else {
            return Err(UpdateManagerError::UnsafePointer);
        };
        if components.next().is_some() {
            return Err(UpdateManagerError::UnsafePointer);
        }
        let target = component
            .to_str()
            .ok_or(UpdateManagerError::UnsafePointer)?;
        if !valid_slot_id(target) {
            return Err(UpdateManagerError::UnsafePointer);
        }
        Ok(Some(target.to_string()))
    }

    pub(crate) fn switch(&self, target_slot_id: &str) -> Result<(), UpdateManagerError> {
        if !valid_slot_id(target_slot_id) {
            return Err(UpdateManagerError::InvalidSlotId);
        }
        self.validate_root()?;
        self.validate_current_node_if_present()?;
        self.remove_stale_temporary()?;
        symlink(target_slot_id, &self.temporary)?;
        let temporary_metadata = fs::symlink_metadata(&self.temporary)?;
        validate_owned_symlink(&temporary_metadata, self.identity)?;
        let result = rename_current(&self.root_directory);
        if let Err(error) = result {
            let _ = self.remove_stale_temporary();
            return Err(UpdateManagerError::Io(error));
        }
        self.root_directory.sync_all()?;
        if self.read()?.as_deref() != Some(target_slot_id) {
            return Err(UpdateManagerError::UnsafePointer);
        }
        Ok(())
    }

    fn validate_root(&self) -> Result<(), UpdateManagerError> {
        if validate_directory(&self.root)? != self.identity
            || identity_from_metadata(&self.root_directory.metadata()?) != self.identity
        {
            return Err(UpdateManagerError::UnsafeSlotRoot);
        }
        Ok(())
    }

    fn validate_current_node_if_present(&self) -> Result<(), UpdateManagerError> {
        match fs::symlink_metadata(&self.current) {
            Ok(metadata) => validate_owned_symlink(&metadata, self.identity),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(UpdateManagerError::Io(error)),
        }
    }

    fn validate_temporary_if_present(&self) -> Result<(), UpdateManagerError> {
        match fs::symlink_metadata(&self.temporary) {
            Ok(metadata) => validate_owned_symlink(&metadata, self.identity),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(UpdateManagerError::Io(error)),
        }
    }

    fn remove_stale_temporary(&self) -> Result<(), UpdateManagerError> {
        match fs::symlink_metadata(&self.temporary) {
            Ok(metadata) => {
                validate_owned_symlink(&metadata, self.identity)?;
                fs::remove_file(&self.temporary)?;
                self.root_directory.sync_all()?;
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(UpdateManagerError::Io(error)),
        }
    }
}

fn rename_current(root: &File) -> io::Result<()> {
    let source = CString::new(TEMPORARY_LINK_NAME).expect("constant has no NUL");
    let destination = CString::new(CURRENT_LINK_NAME).expect("constant has no NUL");
    // SAFETY: both names are fixed NUL-terminated child names and the same
    // owned directory descriptor is used for the atomic replacement.
    let result = unsafe {
        libc::renameat2(
            root.as_raw_fd(),
            source.as_ptr(),
            root.as_raw_fd(),
            destination.as_ptr(),
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn validate_directory(path: &Path) -> Result<PointerIdentity, UpdateManagerError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(UpdateManagerError::InvalidSlotRoot);
    }
    let canonical = fs::canonicalize(path).map_err(|_| UpdateManagerError::UnsafeSlotRoot)?;
    if canonical != path {
        return Err(UpdateManagerError::UnsafeSlotRoot);
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| UpdateManagerError::UnsafeSlotRoot)?;
    let identity = identity_from_metadata(&metadata);
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o022 != 0
        || identity.owner != effective_uid()
    {
        return Err(UpdateManagerError::UnsafeSlotRoot);
    }
    Ok(identity)
}

fn validate_owned_symlink(
    metadata: &fs::Metadata,
    identity: PointerIdentity,
) -> Result<(), UpdateManagerError> {
    if !metadata.file_type().is_symlink()
        || metadata.dev() != identity.device
        || metadata.uid() != identity.owner
        || metadata.nlink() != 1
    {
        return Err(UpdateManagerError::UnsafePointer);
    }
    Ok(())
}

fn identity_from_metadata(metadata: &fs::Metadata) -> PointerIdentity {
    PointerIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        owner: metadata.uid(),
    }
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }
}
