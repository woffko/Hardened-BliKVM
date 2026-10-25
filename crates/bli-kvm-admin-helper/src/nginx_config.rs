//! nginx's configuration before every start: the system's file, with the
//! HTTPS redirect the board's file has (code audit of 2026-10-20, gate Y).
//! A RAW update keeps the new image's file only when the board's own boot
//! step is new enough (Y7); an older one carries the board's file over the
//! new system's, so a board updated from an older release would keep
//! proxying over loopback TCP and miss later TLS settings. The system ships
//! its file outside the carried directory, and this step puts it back with
//! the same rule as the RAW update. A board's file that already is the
//! system's, and a system without a file of its own, are left alone.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Larger than any configuration the system ships.
const MAX_BYTES: u64 = 64 * 1024;

/// Writes the system's configuration over `target` when they differ; true
/// when it wrote.
pub fn render(reference: &Path, target: &Path) -> io::Result<bool> {
    let Some((wanted, _)) = read(reference)? else {
        return Ok(false);
    };
    let Some((current, metadata)) = read(target)? else {
        return Ok(false);
    };
    let wanted = bli_kvm_system_raw_update::kept_nginx_conf(&wanted, &current);
    if wanted == current {
        return Ok(false);
    }
    let directory = target
        .parent()
        .ok_or_else(|| io::Error::other("the configuration has no directory"))?;
    let temporary = directory.join(".nginx.conf.system");
    let _ = fs::remove_file(&temporary);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temporary)?;
    let written = (|| -> io::Result<()> {
        // The board's file keeps its owner and mode.
        // SAFETY: fchown on a descriptor this function owns.
        if unsafe { libc::fchown(file.as_raw_fd(), metadata.uid(), metadata.gid()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        file.set_permissions(fs::Permissions::from_mode(metadata.mode() & 0o7777))?;
        file.write_all(wanted.as_bytes())?;
        file.sync_all()
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    fs::rename(&temporary, target)?;
    File::open(directory)?.sync_all()?;
    Ok(true)
}

/// A small regular file's text, not through a link; None when it is absent.
fn read(path: &Path) -> io::Result<Option<(String, fs::Metadata)>> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return Err(io::Error::other("not a small regular file"));
    }
    let mut text = String::new();
    Read::by_ref(&mut file)
        .take(MAX_BYTES + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > MAX_BYTES {
        return Err(io::Error::other("not a small regular file"));
    }
    Ok(Some((text, metadata)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYSTEM: &str = include_str!("../../../packaging/nginx/hardened-blikvm.conf");

    #[test]
    fn an_older_configuration_is_replaced_by_the_systems() {
        // A board updated by RAW from beta.8: its boot step carried the
        // board's file, which proxies over loopback TCP.
        let dir = tempfile::tempdir().unwrap();
        let reference = dir.path().join("reference.conf");
        let target = dir.path().join("nginx.conf");
        fs::write(&reference, SYSTEM).unwrap();
        let older = SYSTEM.replace(
            "proxy_pass http://hardened_blikvm_server;",
            "proxy_pass http://127.0.0.1:18080;",
        );
        assert_ne!(older, SYSTEM);
        fs::write(&target, &older).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(render(&reference, &target).unwrap());
        assert_eq!(fs::read_to_string(&target).unwrap(), SYSTEM);
        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o600);
        assert!(!dir.path().join(".nginx.conf.system").exists());
        // The system's file is left alone.
        let before = fs::metadata(&target).unwrap().ino();
        assert!(!render(&reference, &target).unwrap());
        assert_eq!(fs::metadata(&target).unwrap().ino(), before);
    }

    #[test]
    fn the_boards_redirect_is_kept() {
        // A system file without the shipped redirect, and the owner's
        // redirect in the board's file: the result redirects, as after Y7.
        let dir = tempfile::tempdir().unwrap();
        let reference = dir.path().join("reference.conf");
        let target = dir.path().join("nginx.conf");
        let plain = SYSTEM.replace("        return 308 https://$host$request_uri;\n", "");
        assert!(!bli_kvm_core::nginx_redirect::redirects_port_80(&plain));
        fs::write(&reference, &plain).unwrap();
        let switched = bli_kvm_core::nginx_redirect::render_redirect(&plain, true);
        fs::write(
            &target,
            switched.replace("hardened_blikvm_server;", "127.0.0.1:18080;"),
        )
        .unwrap();
        assert!(render(&reference, &target).unwrap());
        let rendered = fs::read_to_string(&target).unwrap();
        assert!(bli_kvm_core::nginx_redirect::redirects_port_80(&rendered));
        assert_eq!(rendered, switched);
    }

    #[test]
    fn nothing_is_written_without_both_files_or_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let reference = dir.path().join("reference.conf");
        let target = dir.path().join("nginx.conf");
        // A system without a file of its own.
        fs::write(&target, "older").unwrap();
        assert!(!render(&reference, &target).unwrap());
        assert_eq!(fs::read_to_string(&target).unwrap(), "older");
        // No configuration: nginx does not start, nothing is made.
        fs::write(&reference, SYSTEM).unwrap();
        fs::remove_file(&target).unwrap();
        assert!(!render(&reference, &target).unwrap());
        assert!(!target.exists());
        // A link in the configuration's place is not followed.
        let elsewhere = dir.path().join("elsewhere.conf");
        fs::write(&elsewhere, "older").unwrap();
        std::os::unix::fs::symlink(&elsewhere, &target).unwrap();
        assert!(render(&reference, &target).is_err());
        assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "older");
    }
}
