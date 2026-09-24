//! Point-in-time executable identity, additional to service/IPC readiness.

use std::{
    fs::{File, OpenOptions},
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_ticks: u64,
    pub executable_device: u64,
    pub executable_inode: u64,
}

/// `expected` must be held open from the immutable, signature/hash-verified slot.
/// This function does not authenticate its contents. The caller must obtain PID
/// from the closed unit observation and recheck identity after readiness probes.
/// Permission denial, process exit or ambiguous observations are errors.
pub fn observe_process_identity(pid: u32, expected: &File) -> io::Result<ProcessIdentity> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(invalid());
    }
    let expected = expected.metadata()?;
    if !expected.is_file() {
        return Err(invalid());
    }
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(format!("/proc/{pid}"))?;
    // Keeping this directory open pins the proc entry. A reused PID cannot
    // redirect subsequent lookups to a replacement process's proc directory.
    let anchored = format!("/proc/self/fd/{}", directory.as_raw_fd());
    let path = Path::new(&anchored);
    let before = start_ticks(&path.join("stat"), pid)?;
    for _ in 0..2 {
        // Following the kernel's exe link is intentional; no payload path is used.
        let executable = File::open(path.join("exe"))?;
        let actual = executable.metadata()?;
        if !actual.is_file() || actual.dev() != expected.dev() || actual.ino() != expected.ino() {
            return Err(invalid());
        }
        if start_ticks(&path.join("stat"), pid)? != before {
            return Err(invalid());
        }
    }
    Ok(ProcessIdentity {
        pid,
        start_ticks: before,
        executable_device: expected.dev(),
        executable_inode: expected.ino(),
    })
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "process executable identity is unavailable or changed",
    )
}

fn start_ticks(path: &Path, pid: u32) -> io::Result<u64> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(invalid());
    }
    parse_start_ticks(&bytes, pid)
}

fn parse_start_ticks(bytes: &[u8], pid: u32) -> io::Result<u64> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let (prefix, rest) = text.split_once(" (").ok_or_else(invalid)?;
    if prefix != pid.to_string() {
        return Err(invalid());
    }
    // comm may contain spaces, parentheses and newlines; numeric fields cannot.
    let (_, fields) = rest.rsplit_once(") ").ok_or_else(invalid)?;
    let mut fields = fields.split_ascii_whitespace();
    if !matches!(fields.next(), Some("R" | "S" | "D" | "T" | "t" | "W" | "I")) {
        return Err(invalid());
    }
    // Start time is field 22; state above was field 3.
    let value = fields.nth(18).ok_or_else(invalid)?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    let ticks = value.parse::<u64>().map_err(|_| invalid())?;
    if ticks == 0 {
        return Err(invalid());
    }
    Ok(ticks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchored_proc_directory_does_not_remain_valid_after_child_exit() {
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let expected = File::open("/bin/sleep").unwrap();
        let mut child = Child(
            std::process::Command::new("/bin/sleep")
                .arg("5")
                .env_clear()
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        let pid = child.0.id();
        observe_process_identity(pid, &expected).unwrap();
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(format!("/proc/{pid}"))
            .unwrap();
        let anchored_stat = format!("/proc/self/fd/{}/stat", directory.as_raw_fd());
        start_ticks(Path::new(&anchored_stat), pid).unwrap();
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        assert!(start_ticks(Path::new(&anchored_stat), pid).is_err());
    }

    #[test]
    fn current_process_matches_its_open_executable_but_not_an_unrelated_file() {
        let expected = File::open(std::env::current_exe().unwrap()).unwrap();
        let first = observe_process_identity(std::process::id(), &expected).unwrap();
        assert_eq!(
            observe_process_identity(std::process::id(), &expected).unwrap(),
            first
        );
        let unrelated = tempfile::tempfile().unwrap();
        assert!(observe_process_identity(std::process::id(), &unrelated).is_err());
        assert!(observe_process_identity(0, &expected).is_err());
        assert!(observe_process_identity(u32::MAX, &expected).is_err());
    }

    #[test]
    fn stat_parser_handles_complex_comm_and_rejects_dead_or_invalid_processes() {
        let fields = std::iter::repeat_n("0", 18).collect::<Vec<_>>().join(" ");
        let valid = format!("42 (name ) with\nparentheses)) S {fields} 123 0 0\n");
        assert_eq!(parse_start_ticks(valid.as_bytes(), 42).unwrap(), 123);
        assert!(parse_start_ticks(valid.as_bytes(), 43).is_err());
        for invalid in [
            valid.replace(") S ", ") Z "),
            valid.replace("123", "-1"),
            valid.replace("123", "0"),
            "42 (short) S 0".into(),
        ] {
            assert!(parse_start_ticks(invalid.as_bytes(), 42).is_err());
        }
    }
}
