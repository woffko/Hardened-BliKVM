//! Root helper for switched-on administration features.
//!
//! Socket-activated with `Accept=yes`: the connection is standard input. The
//! helper serves only the configured web-server user, re-reads the owner's
//! switch file for every request and refuses a switched-off feature on its
//! own, whatever the web server decided.

use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use bli_kvm_admin_helper::{
    AdminReply, AdminRequest, FRAME_DATA, FRAME_RESIZE, decode_resize, encode_frame, read_frame,
    read_line, write_line,
};
use bli_kvm_core::{ADMIN_FEATURES_PATH, AdminFeature, AdminFeatureSwitches};

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let peer_user = match (
        arguments.next().as_deref(),
        arguments.next(),
        arguments.next(),
    ) {
        (Some("--peer-user"), Some(user), None) => user,
        _ => {
            eprintln!("usage: bli-kvm-admin-helper --peer-user USER");
            return ExitCode::from(2);
        }
    };
    let mut stream = match authorized_stdin(&peer_user) {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("admin helper: peer rejected: {error}");
            return ExitCode::from(3);
        }
    };
    match serve(&mut stream) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("admin helper: {error}");
            ExitCode::from(1)
        }
    }
}

fn serve(stream: &mut UnixStream) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let request: AdminRequest = read_line(stream)?;
    stream.set_read_timeout(None)?;
    let switches = AdminFeatureSwitches::load(Path::new(ADMIN_FEATURES_PATH));
    match request {
        AdminRequest::Terminal { rows, cols } => {
            if !switches.get(AdminFeature::Terminal) {
                return refuse(stream, "terminal is switched off");
            }
            eprintln!("admin helper: terminal session opened");
            let result = terminal(stream, rows, cols);
            eprintln!("admin helper: terminal session closed");
            result
        }
    }
}

fn refuse(stream: &mut UnixStream, reason: &str) -> io::Result<()> {
    write_line(
        stream,
        &AdminReply::Refused {
            reason: reason.into(),
        },
    )
}

struct Pty {
    master: File,
    child: libc::pid_t,
}

fn set_window(master: &File, rows: u16, cols: u16) {
    let size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCSWINSZ reads a winsize from valid memory on an open PTY.
    unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &size) };
}

/// Opens a PTY and starts a root login shell as the leader of a new session.
fn spawn_shell(rows: u16, cols: u16) -> io::Result<Pty> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    // SAFETY: openpty fills both descriptors on success.
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if opened != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty returned two new descriptors owned here.
    let (master, slave) = unsafe { (File::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    set_window(&master, rows, cols);
    let shell = if Path::new("/bin/bash").exists() {
        "/bin/bash"
    } else {
        "/bin/sh"
    };
    let program = CString::new(shell).map_err(io::Error::other)?;
    let login = CString::new("-l").map_err(io::Error::other)?;
    let environment = [
        "TERM=xterm-256color",
        "HOME=/root",
        "USER=root",
        "LOGNAME=root",
        "SHELL=/bin/bash",
        "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        "LANG=C.UTF-8",
    ]
    .iter()
    .map(|entry| CString::new(*entry).map_err(io::Error::other))
    .collect::<io::Result<Vec<_>>>()?;
    let mut argv = [program.as_ptr(), login.as_ptr(), std::ptr::null()];
    let mut envp: Vec<*const libc::c_char> =
        environment.iter().map(|entry| entry.as_ptr()).collect();
    envp.push(std::ptr::null());

    // SAFETY: the helper is single-threaded at this point; the child only calls
    // async-signal-safe functions before execve.
    let child = unsafe { libc::fork() };
    if child < 0 {
        return Err(io::Error::last_os_error());
    }
    if child == 0 {
        // SAFETY: child side; every call is async-signal-safe and the process
        // either replaces itself with the shell or exits.
        unsafe {
            libc::setsid();
            let slave = slave.as_raw_fd();
            libc::ioctl(slave, libc::TIOCSCTTY, 0);
            libc::dup2(slave, 0);
            libc::dup2(slave, 1);
            libc::dup2(slave, 2);
            if slave > 2 {
                libc::close(slave);
            }
            libc::close(master.as_raw_fd());
            libc::chdir(c"/root".as_ptr());
            argv[0] = program.as_ptr();
            libc::execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr());
            libc::_exit(127);
        }
    }
    drop(slave);
    Ok(Pty { master, child })
}

fn terminal(stream: &mut UnixStream, rows: u16, cols: u16) -> io::Result<()> {
    let (rows, cols) =
        decode_resize(&bli_kvm_admin_helper::encode_resize(rows, cols)).unwrap_or((24, 80));
    let Pty { master, child } = spawn_shell(rows, cols)?;
    write_line(stream, &AdminReply::Ready)?;

    let writer = Arc::new(Mutex::new(stream.try_clone()?));
    let mut output = master.try_clone()?;
    let output_writer = Arc::clone(&writer);
    // Shell output to the client until the PTY closes.
    let pump = thread::spawn(move || {
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            let count = match output.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => count,
            };
            let Ok(frame) = encode_frame(FRAME_DATA, &buffer[..count]) else {
                break;
            };
            let Ok(mut socket) = output_writer.lock() else {
                break;
            };
            if socket.write_all(&frame).is_err() {
                break;
            }
        }
        if let Ok(socket) = output_writer.lock() {
            let _ = socket.shutdown(std::net::Shutdown::Write);
        }
    });

    // Client frames to the shell until the client leaves or the shell exits.
    let mut input = master;
    let mut reader = stream.try_clone()?;
    loop {
        match read_frame(&mut reader) {
            Ok(Some((FRAME_DATA, payload))) => {
                if input.write_all(&payload).is_err() {
                    break;
                }
            }
            Ok(Some((FRAME_RESIZE, payload))) => {
                if let Some((rows, cols)) = decode_resize(&payload) {
                    set_window(&input, rows, cols);
                }
            }
            Ok(Some(_)) | Ok(None) | Err(_) => break,
        }
    }
    // End the whole session: the shell leads its own process group.
    // SAFETY: signals the child's process group and reaps the child.
    unsafe {
        libc::kill(-child, libc::SIGHUP);
        thread::sleep(Duration::from_millis(200));
        let mut status = 0;
        if libc::waitpid(child, &mut status, libc::WNOHANG) == 0 {
            thread::sleep(Duration::from_secs(2));
            libc::kill(-child, libc::SIGKILL);
            libc::waitpid(child, &mut status, 0);
        }
    }
    drop(input);
    let _ = pump.join();
    Ok(())
}

/// The connection on standard input, accepted only from the expected user.
fn authorized_stdin(peer_user: &str) -> io::Result<UnixStream> {
    let valid_name = !peer_user.is_empty()
        && peer_user.len() <= 32
        && peer_user != "root"
        && peer_user.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte)
        });
    if !valid_name {
        return Err(io::Error::other("invalid peer user"));
    }
    let expected = nix::unistd::User::from_name(peer_user)
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("unknown peer user"))?
        .uid
        .as_raw();
    if expected == 0 {
        return Err(io::Error::other("the peer user must not be root"));
    }
    // SAFETY: duplicates stdin without taking ownership of the original FD.
    let duplicate = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_DUPFD_CLOEXEC, 3) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl returned a new uniquely owned descriptor.
    let stream = UnixStream::from(unsafe { OwnedFd::from_raw_fd(duplicate) });
    let mut peer = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut size = size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: SO_PEERCRED initializes the ucred output on a connected Unix stream.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            peer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if result != 0 || size as usize != size_of::<libc::ucred>() {
        return Err(io::Error::other("peer credentials are unavailable"));
    }
    // SAFETY: the successful call initialized every field.
    let peer = unsafe { peer.assume_init() };
    if peer.uid != expected || peer.pid <= 0 {
        return Err(io::Error::other("unexpected peer"));
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_admin_helper::encode_resize;

    #[test]
    fn a_terminal_session_relays_both_ways_and_ends_with_the_client() {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        let session = thread::spawn(move || terminal(&mut server, 24, 80));
        let reply: AdminReply = read_line(&mut client).unwrap();
        assert_eq!(reply, AdminReply::Ready);
        client
            .write_all(&encode_frame(FRAME_RESIZE, &encode_resize(30, 100)).unwrap())
            .unwrap();
        client
            .write_all(&encode_frame(FRAME_DATA, b"echo relay-$((6*7))\r").unwrap())
            .unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut seen = Vec::new();
        while !String::from_utf8_lossy(&seen).contains("relay-42") {
            let (kind, payload) = read_frame(&mut client).unwrap().expect("shell output");
            assert_eq!(kind, FRAME_DATA);
            seen.extend(payload);
        }
        drop(client);
        session.join().unwrap().unwrap();
    }
}
