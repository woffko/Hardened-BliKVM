//! The root password follows the web account's, as in the reference: its
//! first setup and every password change set both (`passwd root` there). The
//! password reaches `chpasswd` on its standard input only, never a command
//! line, the environment or a log.

use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CHPASSWD: &str = "/usr/sbin/chpasswd";
/// The reference allows its `passwd` this long.
const TIMEOUT: Duration = Duration::from_secs(10);

/// A password `chpasswd` takes as one line: the web account's bounds (8 to
/// 256 bytes) and no line break or NUL.
pub fn valid_password(password: &str) -> bool {
    (8..=256).contains(&password.len())
        && !password
            .bytes()
            .any(|byte| matches!(byte, b'\n' | b'\r' | 0))
}

/// The line `chpasswd` reads for root.
pub fn chpasswd_line(password: &str) -> String {
    format!("root:{password}\n")
}

pub fn set(password: &str) -> io::Result<()> {
    if !valid_password(password) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the password is not accepted",
        ));
    }
    let mut child = Command::new(CHPASSWD)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let written = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("chpasswd has no input"))
        .and_then(|mut input| input.write_all(chpasswd_line(password).as_bytes()));
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "chpasswd timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    written?;
    if status.success() {
        Ok(())
    } else {
        // chpasswd names the user and the reason, never the password.
        let mut reason = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = io::Read::read_to_string(&mut stderr, &mut reason);
        }
        Err(io::Error::other(format!(
            "chpasswd failed: {}",
            reason.trim().chars().take(200).collect::<String>()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_are_bounded_single_lines() {
        assert!(valid_password("eight888"));
        assert!(valid_password(&"p".repeat(256)));
        assert!(valid_password("with:colon and spaces"));
        for invalid in ["short", "line\nbreak", "carriage\rreturn", "nul\0byte"] {
            assert!(!valid_password(invalid), "{invalid:?}");
        }
        assert!(!valid_password(&"p".repeat(257)));
        assert_eq!(chpasswd_line("with:colon"), "root:with:colon\n");
    }
}
