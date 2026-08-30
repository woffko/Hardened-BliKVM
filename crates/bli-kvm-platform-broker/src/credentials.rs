use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

use bli_kvm_kvmd_client::{CredentialError, KvmdCredentials};
use thiserror::Error;

const MAX_FILE_BYTES: u64 = 257;

#[derive(Debug, Error)]
pub enum CredentialLoadError {
    #[error("systemd credential directory is invalid")]
    InvalidDirectory,
    #[error("systemd credential file is invalid")]
    InvalidFile,
    #[error("systemd credential file permissions are too broad")]
    BroadPermissions,
    #[error("failed to read systemd credential")]
    Io(#[from] io::Error),
    #[error("KVMD credential content is invalid")]
    InvalidCredential(#[from] CredentialError),
}

pub fn load_systemd_credentials(directory: &Path) -> Result<KvmdCredentials, CredentialLoadError> {
    validate_directory(directory)?;
    let mut username = read_credential(&directory.join("kvmd-user"))?;
    let mut password = read_credential(&directory.join("kvmd-password"))?;
    let credentials = KvmdCredentials::new(&username, &password).map_err(Into::into);
    username.fill(0);
    password.fill(0);
    credentials
}

fn validate_directory(directory: &Path) -> Result<(), CredentialLoadError> {
    if !directory.is_absolute() {
        return Err(CredentialLoadError::InvalidDirectory);
    }
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(CredentialLoadError::InvalidDirectory);
    }
    if metadata.mode() & 0o022 != 0 {
        return Err(CredentialLoadError::BroadPermissions);
    }
    Ok(())
}

fn read_credential(path: &Path) -> Result<Vec<u8>, CredentialLoadError> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)?;
    validate_file(&file)?;
    let mut value = Vec::new();
    file.by_ref()
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut value)?;
    if value.len() as u64 > MAX_FILE_BYTES {
        value.fill(0);
        return Err(CredentialLoadError::InvalidFile);
    }
    if value.last() == Some(&b'\n') {
        value.pop();
        if value.last() == Some(&b'\r') {
            value.pop();
        }
    }
    Ok(value)
}

fn validate_file(file: &File) -> Result<(), CredentialLoadError> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err(CredentialLoadError::InvalidFile);
    }
    if metadata.mode() & 0o077 != 0 {
        return Err(CredentialLoadError::BroadPermissions);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs::Permissions, os::unix::fs::PermissionsExt};

    use super::*;

    fn write_private(path: &Path, value: &[u8]) {
        fs::write(path, value).unwrap();
        fs::set_permissions(path, Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn loads_private_bounded_test_credentials() {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), Permissions::from_mode(0o700)).unwrap();
        write_private(&directory.path().join("kvmd-user"), b"test-user\n");
        write_private(
            &directory.path().join("kvmd-password"),
            b"not-a-real-password\n",
        );
        let _credentials = load_systemd_credentials(directory.path()).unwrap();
    }

    #[test]
    fn rejects_world_readable_or_symlinked_credentials() {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), Permissions::from_mode(0o700)).unwrap();
        let user = directory.path().join("kvmd-user");
        let password = directory.path().join("kvmd-password");
        write_private(&user, b"test-user");
        write_private(&password, b"not-a-real-password");
        fs::set_permissions(&password, Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            load_systemd_credentials(directory.path()),
            Err(CredentialLoadError::BroadPermissions)
        ));

        fs::remove_file(&password).unwrap();
        std::os::unix::fs::symlink(&user, &password).unwrap();
        assert!(matches!(
            load_systemd_credentials(directory.path()),
            Err(CredentialLoadError::Io(error)) if error.raw_os_error() == Some(libc::ELOOP)
        ));
    }
}
