use std::{
    fs::OpenOptions,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundedSysfsReadError {
    Unavailable,
    UnsafeSource,
    InvalidValue,
}

pub(crate) fn read_bounded_ascii(
    path: &Path,
    maximum_bytes: u64,
) -> Result<String, BoundedSysfsReadError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| {
            if error.raw_os_error() == Some(libc::ELOOP) {
                BoundedSysfsReadError::UnsafeSource
            } else {
                BoundedSysfsReadError::Unavailable
            }
        })?;
    let metadata = file
        .metadata()
        .map_err(|_| BoundedSysfsReadError::UnsafeSource)?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.permissions().mode() & 0o022 != 0 {
        return Err(BoundedSysfsReadError::UnsafeSource);
    }
    let mut bytes = Vec::with_capacity(maximum_bytes as usize);
    file.take(maximum_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| BoundedSysfsReadError::Unavailable)?;
    if bytes.is_empty() || bytes.len() as u64 > maximum_bytes {
        return Err(BoundedSysfsReadError::InvalidValue);
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    if bytes.is_empty() || !bytes.is_ascii() {
        return Err(BoundedSysfsReadError::InvalidValue);
    }
    String::from_utf8(bytes).map_err(|_| BoundedSysfsReadError::InvalidValue)
}
