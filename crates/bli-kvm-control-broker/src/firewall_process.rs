//! Bounded capture for the owner's helper processes (the firewall's, and
//! systemd's and the other system commands the owner runs); no shell or
//! reader threads. A helper past its deadline is killed.
use std::{
    io::{self, Read},
    os::fd::AsRawFd,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    // SAFETY: fd belongs to the live owned pipe; these fcntl commands take only
    // integer arguments and neither dereference memory nor transfer ownership.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the same owned fd remains live and flags are the retrieved flags
    // plus the platform-provided nonblocking bit.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn read_once(pipe: &mut impl Read, output: &mut Vec<u8>, limit: usize) -> Result<bool, String> {
    let mut buffer = [0; 4096];
    match pipe.read(&mut buffer) {
        Ok(0) => Ok(true),
        Ok(count) => {
            if count > limit.saturating_sub(output.len()) {
                return Err("helper output exceeds bound".into());
            }
            output.extend_from_slice(&buffer[..count]);
            Ok(false)
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn run(mut command: Command, timeout: Duration, limit: usize) -> Result<String, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ChildGuard(command.spawn().map_err(|error| error.to_string())?);
    let mut stdout = child.0.stdout.take().ok_or("missing helper stdout")?;
    let mut stderr = child.0.stderr.take().ok_or("missing helper stderr")?;
    nonblocking(&stdout).map_err(|error| error.to_string())?;
    nonblocking(&stderr).map_err(|error| error.to_string())?;
    let start = Instant::now();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (mut out_eof, mut err_eof) = (false, false);
    loop {
        if start.elapsed() >= timeout {
            return Err("helper timed out".into());
        }
        if !out_eof {
            out_eof = read_once(&mut stdout, &mut out, limit)?;
        }
        if !err_eof {
            err_eof = read_once(&mut stderr, &mut err, limit)?;
        }
        if let Some(status) = child.0.try_wait().map_err(|error| error.to_string())? {
            if out_eof && err_eof {
                if !status.success() {
                    return Err(if err.is_empty() {
                        "helper failed".into()
                    } else {
                        String::from_utf8_lossy(&err).trim().to_string()
                    });
                }
                return String::from_utf8(out).map_err(|_| "invalid helper output encoding".into());
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn python(script: &str) -> Command {
        let mut command = Command::new("/usr/bin/python3");
        command.args(["-c", script]);
        command
    }
    #[test]
    fn captures_success_and_reports_failed_exit() {
        assert_eq!(
            run(python("print('fixture')"), Duration::from_secs(2), 1024).unwrap(),
            "fixture\n"
        );
        let error = run(
            python("import sys; sys.stderr.write('fixture failure'); sys.exit(3)"),
            Duration::from_secs(2),
            1024,
        )
        .unwrap_err();
        assert_eq!(error, "fixture failure");
    }
    #[test]
    fn excessive_output_and_stalled_children_are_bounded() {
        assert!(
            run(python("print('x' * 4096)"), Duration::from_secs(2), 128)
                .unwrap_err()
                .contains("exceeds bound")
        );
        let start = Instant::now();
        assert!(
            run(
                python("import time; time.sleep(10)"),
                Duration::from_millis(50),
                128
            )
            .unwrap_err()
            .contains("timed out")
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
