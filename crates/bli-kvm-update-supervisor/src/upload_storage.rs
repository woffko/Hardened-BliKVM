//! Private upload admission and anonymous spool, separate from the slot lock.

use std::{
    ffi::CString,
    fs::File,
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path},
};

/// Retaining this guard admits at most one upload in this private directory.
pub struct UploadStorage {
    directory: File,
}

impl UploadStorage {
    /// The administrator must provision an updater-owned directory with 0700.
    /// Every component is opened without following symlinks and pinned by FD.
    pub fn open(path: &Path) -> io::Result<Self> {
        let directory = open_directory(path)?;
        let metadata = directory.metadata()?;
        // SAFETY: geteuid has no arguments or memory preconditions.
        let uid = unsafe { libc::geteuid() };
        if metadata.uid() != uid || metadata.mode() & 0o7777 != 0o700 {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        // Lock the directory inode, not a replaceable lock-file name. This is
        // intentionally unrelated to the slot-root lock needed by watchdog.
        // SAFETY: the borrowed descriptor remains valid throughout this call.
        if unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { directory })
    }

    pub fn create_spool(&self, archive_bytes: u64) -> io::Result<File> {
        if archive_bytes == 0
            || archive_bytes > bli_kvm_update_protocol::upload::MAX_UPLOAD_ARCHIVE_BYTES
        {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        // This is a conservative availability check, not a reservation against
        // unrelated writers. Later staging must separately check its own FS.
        check_capacity(&self.directory, archive_bytes, 64)?;
        // O_TMPFILE never exposes a pathname, including during abnormal exit.
        // No fallback to a named file on filesystems lacking this guarantee.
        // SAFETY: the directory FD and constant nul-terminated name are valid.
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                c".".as_ptr(),
                libc::O_TMPFILE | libc::O_RDWR | libc::O_CLOEXEC | libc::O_EXCL,
                0o600,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful openat returned a uniquely owned descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() != 0 || metadata.mode() & 0o7777 != 0o600 {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        Ok(file)
    }
}

/// Check staging filesystem capacity while the caller holds the stager lock.
/// Reserve includes snapshot, extraction and fixed free-byte/inode headroom.
/// A separate upload spool may reside on another filesystem.
pub fn check_staging_capacity(path: &Path, archive_bytes: u64) -> io::Result<()> {
    let directory = open_directory(path)?;
    let peak = archive_bytes
        .checked_mul(2)
        .ok_or(io::ErrorKind::InvalidInput)?;
    check_capacity(
        &directory,
        peak,
        bli_kvm_update_verifier::MAX_PAYLOAD_FILES as u64 * 2 + 64,
    )
}

fn check_capacity(directory: &File, required: u64, required_inodes: u64) -> io::Result<()> {
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: fstatvfs initializes the correctly sized output for this valid FD.
    if unsafe { libc::fstatvfs(directory.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful call initialized the output structure.
    let stats = unsafe { stats.assume_init() };
    let free_bytes = u128::from(stats.f_bavail) * u128::from(stats.f_frsize);
    let allocation_headroom = u128::from(required_inodes) * u128::from(stats.f_frsize);
    if free_bytes < u128::from(required) + allocation_headroom + 64 * 1024 * 1024
        || u128::from(stats.f_favail) < u128::from(required_inodes)
    {
        return Err(io::Error::from_raw_os_error(libc::ENOSPC));
    }
    Ok(())
}

fn open_directory(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut directory = File::open("/")?;
    let mut normal = false;
    for component in path.components() {
        let Component::Normal(name) = component else {
            if component == Component::RootDir {
                continue;
            }
            return Err(io::ErrorKind::InvalidInput.into());
        };
        normal = true;
        let name = CString::new(name.as_bytes()).map_err(|_| io::ErrorKind::InvalidInput)?;
        // SAFETY: both FD and nul-terminated name remain valid during openat.
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful openat returned a new uniquely owned descriptor.
        directory = unsafe { File::from_raw_fd(fd) };
    }
    if !normal {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{Read, Seek, Write},
        os::unix::fs::{PermissionsExt, symlink},
    };

    #[test]
    fn single_admission_anonymous_file_and_release() {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let storage = UploadStorage::open(root.path()).unwrap();
        assert!(UploadStorage::open(root.path()).is_err());
        let mut file = storage.create_spool(3).unwrap();
        file.write_all(b"abc").unwrap();
        file.rewind().unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"abc");
        assert_eq!(file.metadata().unwrap().nlink(), 0);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
        drop(file);
        drop(storage);
        assert!(UploadStorage::open(root.path()).is_ok());
    }

    #[test]
    fn rejects_exposed_directories_symlinks_and_bad_lengths() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("spool");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(UploadStorage::open(&path).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        symlink(&path, root.path().join("alias")).unwrap();
        assert!(UploadStorage::open(&root.path().join("alias")).is_err());
        let storage = UploadStorage::open(&path).unwrap();
        assert!(storage.create_spool(0).is_err());
        assert!(storage.create_spool(u64::MAX).is_err());
        assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
    }
}
