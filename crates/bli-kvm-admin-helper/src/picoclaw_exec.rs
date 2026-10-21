//! The copy of PicoClaw's executable the runtime starts (code audit of
//! 2026-10-20, AUD-05.2). The installed file is written by an unprivileged
//! account; before every start root checks it against the pinned release's
//! size and digest and copies it into a directory of root's, which the
//! runtime's unit starts it from. A file that differs is not started, and no
//! earlier copy is left to start either.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use sha2::{Digest, Sha256};

/// Name of the copy in its directory.
pub const COPY: &str = "picoclaw";

pub fn prepare(installed: &Path, directory: &Path, sha256: &str, bytes: u64) -> io::Result<()> {
    let own = nix_euid();
    match fs::symlink_metadata(directory) {
        Ok(metadata)
            if metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == own
                && metadata.mode() & 0o022 == 0 => {}
        Ok(_) => return Err(io::Error::other("the copy's directory is not root's alone")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new().mode(0o755).create(directory)?;
        }
        Err(error) => return Err(error),
    }
    // An earlier copy is not left to start if this one is refused.
    match fs::remove_file(directory.join(COPY)) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut source = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(installed)?;
    let metadata = source.metadata()?;
    if !metadata.is_file() || metadata.len() != bytes {
        return Err(io::Error::other(
            "the installed PicoClaw is not the pinned release's",
        ));
    }
    let temporary = directory.join(".picoclaw.new");
    let _ = fs::remove_file(&temporary);
    let mut copy = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temporary)?;
    let copied = (|| -> io::Result<()> {
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; 1024 * 1024];
        let mut total = 0_u64;
        loop {
            let count = source.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > bytes {
                break;
            }
            hasher.update(&buffer[..count]);
            copy.write_all(&buffer[..count])?;
        }
        let digest: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if total != bytes || digest != sha256 {
            return Err(io::Error::other(
                "the installed PicoClaw is not the pinned release's",
            ));
        }
        copy.set_permissions(fs::Permissions::from_mode(0o755))?;
        copy.sync_all()
    })();
    if let Err(error) = copied {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    fs::rename(&temporary, directory.join(COPY))?;
    File::open(directory)?.sync_all()
}

fn nix_euid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    #[test]
    fn only_the_pinned_executable_is_copied_to_be_started() {
        let dir = tempfile::tempdir().unwrap();
        let installed = dir.path().join("installed");
        let exec = dir.path().join("exec");
        fs::write(&installed, b"the pinned runtime").unwrap();
        let (sha256, bytes) = (digest(b"the pinned runtime"), 18);
        prepare(&installed, &exec, &sha256, bytes).unwrap();
        assert_eq!(fs::read(exec.join(COPY)).unwrap(), b"the pinned runtime");
        let mode = fs::metadata(exec.join(COPY)).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o755);
        // Replaced by another account: not copied, and the earlier copy is
        // gone, so nothing starts.
        fs::write(&installed, b"another runtime!!!").unwrap();
        assert!(prepare(&installed, &exec, &sha256, bytes).is_err());
        assert!(!exec.join(COPY).exists());
        // A link in its place is not followed.
        fs::remove_file(&installed).unwrap();
        fs::write(dir.path().join("pinned"), b"the pinned runtime").unwrap();
        std::os::unix::fs::symlink(dir.path().join("pinned"), &installed).unwrap();
        assert!(prepare(&installed, &exec, &sha256, bytes).is_err());
        // Nor is a directory others may write used.
        let shared = dir.path().join("shared");
        fs::create_dir(&shared).unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(prepare(&dir.path().join("pinned"), &shared, &sha256, bytes).is_err());
    }
}
