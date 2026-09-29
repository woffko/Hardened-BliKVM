//! Stored scripts, run as root while the scripts switch is on.
//!
//! Scripts live in a root-only directory, are named as the reference accepts,
//! are limited in size and count, and run with a bounded time and output in
//! the foreground or as a detached transient unit with a runtime limit.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use bli_kvm_admin_helper::{
    AdminReply, FRAME_DATA, MAX_SCRIPT_BYTES, MAX_SCRIPT_OUTPUT_BYTES, MAX_SCRIPTS, read_frame,
    valid_script_name,
};

pub const SCRIPT_DIRECTORY: &str = "/var/lib/hardened-blikvm-admin/scripts";
const FOREGROUND_TIMEOUT: Duration = Duration::from_secs(60);
const BACKGROUND_LIMIT_SECONDS: u32 = 3600;

pub struct ScriptStore {
    root: PathBuf,
}

impl ScriptStore {
    /// Opens (and if needed creates) the root-only script directory.
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        if let Some(parent) = root.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::symlink_metadata(&root) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(io::Error::other("the script directory is not a directory")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(&root)?,
            Err(error) => return Err(error),
        }
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        Ok(Self { root })
    }

    fn path(&self, name: &str) -> io::Result<PathBuf> {
        if !valid_script_name(name) {
            return Err(io::Error::other("invalid script name"));
        }
        Ok(self.root.join(name))
    }

    fn existing(&self, name: &str) -> io::Result<PathBuf> {
        let path = self.path(name)?;
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(io::Error::other("not a script file"));
        }
        Ok(path)
    }

    pub fn list(&self) -> io::Result<Vec<String>> {
        let mut files = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            if let Some(name) = entry.file_name().to_str()
                && valid_script_name(name)
            {
                files.push(name.to_string());
            }
        }
        files.sort();
        Ok(files)
    }

    pub fn store(&self, name: &str, content: &[u8]) -> io::Result<()> {
        let path = self.path(name)?;
        if content.is_empty() || content.len() > MAX_SCRIPT_BYTES {
            return Err(io::Error::other("script size out of bounds"));
        }
        if !path.exists() && self.list()?.len() >= MAX_SCRIPTS {
            return Err(io::Error::other("too many scripts"));
        }
        let temporary = self.root.join(format!(".{name}.new"));
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o700)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &path)
    }

    pub fn delete(&self, name: &str) -> io::Result<()> {
        fs::remove_file(self.existing(name)?)
    }

    fn command(&self, name: &str) -> io::Result<(String, PathBuf)> {
        let path = self.existing(name)?;
        let interpreter = if name.to_ascii_lowercase().ends_with(".py") {
            "/usr/bin/python3"
        } else {
            "/bin/sh"
        };
        Ok((interpreter.to_string(), path))
    }

    /// Runs in the foreground with a time limit; stdout then stderr, each
    /// capped, as the reference returns them.
    pub fn run_foreground(&self, name: &str) -> io::Result<AdminReply> {
        self.run_foreground_within(name, FOREGROUND_TIMEOUT)
    }

    fn run_foreground_within(&self, name: &str, limit: Duration) -> io::Result<AdminReply> {
        let (interpreter, path) = self.command(name)?;
        // A group of its own, so that what the script leaves behind can be
        // killed with it: a background child holds the output pipes open and
        // would keep this call waiting long after the script has finished.
        let mut child = Command::new(interpreter)
            .arg(&path)
            .current_dir("/")
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = capture(child.stdout.take());
        let stderr = capture(child.stderr.take());
        let started = Instant::now();
        let group = i32::try_from(child.id()).unwrap_or(0);
        let kill_group = || {
            if group > 0 {
                // SAFETY: killpg only signals the process group this call
                // created for the script.
                unsafe { libc::killpg(group, libc::SIGKILL) };
            }
        };
        let status = loop {
            if let Some(status) = child.try_wait()? {
                kill_group();
                break Some(status);
            }
            if started.elapsed() >= limit {
                kill_group();
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            thread::sleep(Duration::from_millis(50));
        };
        let mut log = stdout.join().unwrap_or_default();
        log.extend(stderr.join().unwrap_or_default());
        let mut log = String::from_utf8_lossy(&log).into_owned();
        if status.is_none() {
            log.push_str("\n[script stopped after 60 seconds]\n");
        }
        Ok(AdminReply::Output {
            log,
            success: status.is_some_and(|status| status.success()),
        })
    }

    /// Starts the script as a transient unit that systemd stops after an hour.
    pub fn run_background(&self, name: &str) -> io::Result<AdminReply> {
        let (interpreter, path) = self.command(name)?;
        let unit = format!(
            "hardened-blikvm-script-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|now| now.as_millis())
                .unwrap_or(0)
        );
        let status = Command::new("/usr/bin/systemd-run")
            .args([
                "--quiet",
                "--collect",
                &format!("--unit={unit}"),
                &format!("--property=RuntimeMaxSec={BACKGROUND_LIMIT_SECONDS}"),
                "--property=WorkingDirectory=/",
            ])
            .arg(interpreter)
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(io::Error::other("the background script did not start"));
        }
        Ok(AdminReply::Done)
    }
}

/// Reads at most the output limit, draining the rest so the child never
/// blocks on a full pipe.
fn capture(stream: Option<impl Read + Send + 'static>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut kept = Vec::new();
        let Some(mut stream) = stream else {
            return kept;
        };
        let mut buffer = [0_u8; 8192];
        while let Ok(count) = stream.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let room = MAX_SCRIPT_OUTPUT_BYTES.saturating_sub(kept.len());
            kept.extend_from_slice(&buffer[..count.min(room)]);
        }
        kept
    })
}

/// Reads exactly `size` bytes of data frames for an upload.
pub fn read_upload(reader: &mut impl Read, size: u32) -> io::Result<Vec<u8>> {
    let size = size as usize;
    if size == 0 || size > MAX_SCRIPT_BYTES {
        return Err(io::Error::other("script size out of bounds"));
    }
    let mut content = Vec::with_capacity(size);
    while content.len() < size {
        match read_frame(reader)? {
            Some((FRAME_DATA, payload)) if content.len() + payload.len() <= size => {
                content.extend(payload);
            }
            _ => return Err(io::Error::other("invalid script upload")),
        }
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_admin_helper::encode_frame;

    #[test]
    fn scripts_are_stored_listed_run_and_deleted_within_limits() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScriptStore::open(dir.path().join("scripts")).unwrap();
        assert!(store.list().unwrap().is_empty());
        store
            .store("hello.sh", b"echo out-$((6*7)); echo err >&2; exit 3\n")
            .unwrap();
        assert!(store.store("../escape.sh", b"x").is_err());
        assert!(
            store
                .store("big.sh", &vec![b'#'; MAX_SCRIPT_BYTES + 1])
                .is_err()
        );
        assert!(store.store("empty.sh", b"").is_err());
        assert_eq!(store.list().unwrap(), ["hello.sh"]);
        let mode = fs::metadata(dir.path().join("scripts/hello.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
        match store.run_foreground("hello.sh").unwrap() {
            AdminReply::Output { log, success } => {
                assert!(log.contains("out-42") && log.contains("err"), "{log}");
                assert!(!success);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(store.run_foreground("missing.sh").is_err());
        store.delete("hello.sh").unwrap();
        assert!(store.delete("hello.sh").is_err());
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn a_background_child_does_not_hold_the_foreground_run() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScriptStore::open(dir.path().join("scripts")).unwrap();
        store
            .store("bg.sh", b"sleep 30 &\necho started\n")
            .unwrap();
        let begun = Instant::now();
        match store.run_foreground("bg.sh").unwrap() {
            AdminReply::Output { log, success } => {
                assert!(log.contains("started") && success, "{log}");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(begun.elapsed() < Duration::from_secs(5), "{:?}", begun.elapsed());
    }

    #[test]
    fn the_time_limit_kills_the_whole_group() {
        let dir = tempfile::tempdir().unwrap();
        let store = ScriptStore::open(dir.path().join("scripts")).unwrap();
        store
            .store(
                "stubborn.sh",
                b"trap '' TERM\n(trap '' TERM; sleep 30) &\nsleep 30\n",
            )
            .unwrap();
        let begun = Instant::now();
        match store
            .run_foreground_within("stubborn.sh", Duration::from_millis(600))
            .unwrap()
        {
            AdminReply::Output { log, success } => {
                assert!(log.contains("stopped") && !success, "{log}");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(begun.elapsed() < Duration::from_secs(5), "{:?}", begun.elapsed());
    }

    #[test]
    fn uploads_read_exactly_the_announced_frames() {
        let mut stream = encode_frame(FRAME_DATA, b"echo ").unwrap();
        stream.extend(encode_frame(FRAME_DATA, b"hi\n").unwrap());
        assert_eq!(
            read_upload(&mut stream.as_slice(), 8).unwrap(),
            b"echo hi\n"
        );
        let too_long = encode_frame(FRAME_DATA, b"123456789").unwrap();
        assert!(read_upload(&mut too_long.as_slice(), 8).is_err());
        assert!(read_upload(&mut [].as_slice(), 0).is_err());
    }
}
