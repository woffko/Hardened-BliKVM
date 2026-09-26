//! Root helper for switched-on administration features.
//!
//! Socket-activated with `Accept=yes`: the connection is standard input. The
//! helper serves only the configured web-server user, re-reads the owner's
//! switch file for every request and refuses a switched-off feature on its
//! own, whatever the web server decided.
//!
//! With `--download` the same binary serves remote image downloads and the
//! PicoClaw install from a separate unprivileged unit that has network
//! access; with `--picoclaw` it manages the PicoClaw configuration as the
//! runtime's own user; `--require-feature NAME` exits non-zero while that
//! switch is off (a pre-start check for the PicoClaw runtime).

mod download;
mod picoclaw;
mod scripts;
mod system_update;
mod tailscale;

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
    AdminReply, AdminRequest, DOWNLOAD_STATUS, DownloadStatus, FRAME_DATA, FRAME_RESIZE,
    PICOCLAW_BINARY, PICOCLAW_INSTALL_STATUS, PICOCLAW_RELEASE_BYTES, PICOCLAW_RELEASE_SHA256,
    PICOCLAW_RELEASE_URL, PICOCLAW_RUNTIME_DIR, PICOCLAW_SERVICE, PICOCLAW_STATE, decode_resize,
    encode_frame, read_frame, read_line, validate_remote_iso, write_line, write_reply,
};
use bli_kvm_core::{ADMIN_FEATURES_PATH, AdminFeature, AdminFeatureSwitches};

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let mut mode = "admin";
    let mut first = arguments.next();
    match first.as_deref() {
        Some("--download") => {
            mode = "download";
            first = arguments.next();
        }
        Some("--picoclaw") => {
            mode = "picoclaw";
            first = arguments.next();
        }
        Some("--require-feature") => {
            let feature = arguments.next();
            let enabled = AdminFeature::ALL.into_iter().find(|candidate| {
                Some(candidate.key()) == feature.as_deref() && arguments.next().is_none()
            });
            return match enabled {
                Some(feature)
                    if AdminFeatureSwitches::load(Path::new(ADMIN_FEATURES_PATH)).get(feature) =>
                {
                    ExitCode::SUCCESS
                }
                Some(feature) => {
                    eprintln!("admin helper: {} is switched off", feature.key());
                    ExitCode::from(1)
                }
                None => ExitCode::from(2),
            };
        }
        _ => {}
    }
    let peer_user = match (first.as_deref(), arguments.next(), arguments.next()) {
        (Some("--peer-user"), Some(user), None) => user,
        _ => {
            eprintln!(
                "usage: bli-kvm-admin-helper [--download | --picoclaw] --peer-user USER\n       bli-kvm-admin-helper --require-feature NAME"
            );
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
    let served = match mode {
        "download" => serve_download(&mut stream),
        "picoclaw" => serve_picoclaw(&mut stream),
        _ => serve(&mut stream),
    };
    match served {
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
        AdminRequest::PicoclawService { start } => {
            if start && !switches.get(AdminFeature::Picoclaw) {
                return refuse(stream, "picoclaw is switched off");
            }
            let reply = picoclaw_service(start).unwrap_or_else(|error| AdminReply::Refused {
                reason: error.to_string(),
            });
            write_line(stream, &reply)
        }
        AdminRequest::SystemUpdateStatus {} => {
            let reply = system_update::status()
                .map(|update| AdminReply::SystemUpdate { update })
                .unwrap_or_else(|error| AdminReply::Refused {
                    reason: error.to_string(),
                });
            write_line(stream, &reply)
        }
        AdminRequest::SystemUpdateCancel {} => {
            let reply = system_update::cancel_update()
                .map(|update| AdminReply::SystemUpdate { update })
                .unwrap_or_else(|error| AdminReply::Refused {
                    reason: error.to_string(),
                });
            write_line(stream, &reply)
        }
        request @ (AdminRequest::SystemUpdateUpload { .. }
        | AdminRequest::SystemUpdateConfirm {}
        | AdminRequest::SystemUpdateRollback {}) => {
            if !switches.get(AdminFeature::RawSystemUpdate) {
                return refuse(stream, "raw system updates are switched off");
            }
            let reply = system_update_request(stream, request).unwrap_or_else(|error| {
                AdminReply::Refused {
                    reason: error.to_string(),
                }
            });
            write_line(stream, &reply)
        }
        AdminRequest::Tailscale { action } => {
            eprintln!("admin helper: tailscale {action}");
            let reply = tailscale_action(&action).unwrap_or_else(|error| AdminReply::Refused {
                reason: error.to_string(),
            });
            write_line(stream, &reply)
        }
        request @ (AdminRequest::ScriptList {}
        | AdminRequest::ScriptUpload { .. }
        | AdminRequest::ScriptDelete { .. }
        | AdminRequest::ScriptRun { .. }) => {
            if !switches.get(AdminFeature::Scripts) {
                return refuse(stream, "scripts are switched off");
            }
            let reply = script(stream, request).unwrap_or_else(|error| AdminReply::Refused {
                reason: error.to_string(),
            });
            write_line(stream, &reply)
        }
        _ => refuse(stream, "not an administration request"),
    }
}

/// Uploads, confirmations and rollbacks of the RAW system update, with the
/// switch already checked.
fn system_update_request(stream: &mut UnixStream, request: AdminRequest) -> io::Result<AdminReply> {
    let update = match request {
        AdminRequest::SystemUpdateUpload { size } => {
            system_update::check_upload(size)?;
            write_line(stream, &AdminReply::Ready)?;
            // A stalled upload must not hold the staging directory forever.
            stream.set_read_timeout(Some(Duration::from_secs(60)))?;
            eprintln!("admin helper: receiving a system bundle of {size} bytes");
            let upload = system_update::receive(stream, size)?;
            system_update::stage_upload(&upload)?
        }
        AdminRequest::SystemUpdateConfirm {} => system_update::confirm_update()?,
        AdminRequest::SystemUpdateRollback {} => system_update::rollback_update()?,
        _ => return Err(io::Error::other("not a system update request")),
    };
    Ok(AdminReply::SystemUpdate { update })
}

/// The reference's Tailscale actions; no switch, as in the reference.
fn tailscale_action(action: &str) -> io::Result<AdminReply> {
    let done = |result: io::Result<()>| result.map(|()| AdminReply::Done);
    match action {
        "status" => Ok(AdminReply::Tailscale {
            tailscale: tailscale::status(),
        }),
        "login" => tailscale::login().map(|url| AdminReply::TailscaleLogin { url }),
        "install" => done(tailscale::install()),
        "uninstall" => done(tailscale::uninstall()),
        "start" => done(tailscale::start()),
        "stop" => done(tailscale::stop()),
        "restart" => done(tailscale::restart()),
        "up" => done(tailscale::up()),
        "down" => done(tailscale::down()),
        "logout" => done(tailscale::logout()),
        _ => Err(io::Error::other("unknown tailscale action")),
    }
}

/// Starts or stops the PicoClaw runtime unit; its own pre-start check
/// refuses again while the switch is off.
fn picoclaw_service(start: bool) -> io::Result<AdminReply> {
    let mut child = std::process::Command::new("/usr/bin/systemctl")
        .args([if start { "start" } else { "stop" }, PICOCLAW_SERVICE])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("the PicoClaw runtime did not respond"));
        }
        thread::sleep(Duration::from_millis(100));
    };
    eprintln!(
        "admin helper: picoclaw runtime {}",
        if start { "started" } else { "stopped" }
    );
    if status.success() {
        Ok(AdminReply::Done)
    } else {
        Err(io::Error::other(if start {
            "the PicoClaw runtime did not start"
        } else {
            "the PicoClaw runtime did not stop"
        }))
    }
}

fn script(stream: &mut UnixStream, request: AdminRequest) -> io::Result<AdminReply> {
    let store = scripts::ScriptStore::open(scripts::SCRIPT_DIRECTORY)?;
    match request {
        AdminRequest::ScriptList {} => Ok(AdminReply::Scripts {
            files: store.list()?,
        }),
        AdminRequest::ScriptUpload { name, size } => {
            stream.set_read_timeout(Some(Duration::from_secs(30)))?;
            let content = scripts::read_upload(stream, size)?;
            store.store(&name, &content)?;
            eprintln!("admin helper: script {name} stored ({size} bytes)");
            Ok(AdminReply::Done)
        }
        AdminRequest::ScriptDelete { name } => {
            store.delete(&name)?;
            eprintln!("admin helper: script {name} deleted");
            Ok(AdminReply::Done)
        }
        AdminRequest::ScriptRun { name, background } => {
            eprintln!(
                "admin helper: script {name} run in the {}",
                if background {
                    "background"
                } else {
                    "foreground"
                }
            );
            if background {
                store.run_background(&name)
            } else {
                store.run_foreground(&name)
            }
        }
        _ => Err(io::Error::other("not a script request")),
    }
}

fn remote_download_enabled() -> bool {
    AdminFeatureSwitches::load(Path::new(ADMIN_FEATURES_PATH)).get(AdminFeature::RemoteDownload)
}

/// Accepts one download, answers at once and keeps the image transfer going
/// after the web server has hung up; progress goes to the status file.
fn serve_download(stream: &mut UnixStream) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let request: AdminRequest = read_line(stream)?;
    let url = match request {
        AdminRequest::ImageDownload { url } => url,
        AdminRequest::PicoclawInstall {} => return install_picoclaw(stream),
        AdminRequest::PicoclawRemove {} => return remove_picoclaw(stream),
        _ => return refuse(stream, "not a download request"),
    };
    if !remote_download_enabled() {
        return refuse(stream, "remote download is switched off");
    }
    let Some(iso) = validate_remote_iso(&url) else {
        return refuse(stream, "invalid url");
    };
    let status = Path::new(DOWNLOAD_STATUS);
    let lock_path = status.with_file_name("download.lock");
    let Some(_lock) = download::DownloadLock::acquire(&lock_path)? else {
        return refuse(stream, "download in progress");
    };
    download::write_status(status, &download::in_progress(&iso.url, ""))?;
    write_line(stream, &AdminReply::Ready)?;
    let _ = stream.shutdown(std::net::Shutdown::Both);
    eprintln!("admin helper: download of {} started", iso.filename);
    let job = download::Download {
        iso: &iso,
        images: Path::new(download::IMAGE_DIRECTORY),
        catalog: Path::new(download::MEDIA_CATALOG),
        status,
        curl: Path::new(download::CURL),
    };
    match job.run(remote_download_enabled) {
        Ok(()) => {
            eprintln!("admin helper: download of {} finished", iso.filename);
            download::write_status(status, &DownloadStatus::idle())
        }
        Err(error) => {
            eprintln!("admin helper: download of {} failed: {error}", iso.filename);
            let failed = DownloadStatus {
                status: "failed".into(),
                file: iso.url.clone(),
                percentage: String::new(),
            };
            download::write_status(status, &failed)
        }
    }
}

fn picoclaw_enabled() -> bool {
    AdminFeatureSwitches::load(Path::new(ADMIN_FEATURES_PATH)).get(AdminFeature::Picoclaw)
}

/// Installs the pinned PicoClaw release; one network job at a time.
fn install_picoclaw(stream: &mut UnixStream) -> io::Result<()> {
    if !picoclaw_enabled() {
        return refuse(stream, "picoclaw is switched off");
    }
    let status = Path::new(DOWNLOAD_STATUS);
    let lock_path = status.with_file_name("download.lock");
    let Some(_lock) = download::DownloadLock::acquire(&lock_path)? else {
        return refuse(stream, "a download is in progress");
    };
    let install_status = Path::new(PICOCLAW_INSTALL_STATUS);
    download::write_install_status(install_status, "downloading", 0, "");
    write_line(stream, &AdminReply::Ready)?;
    let _ = stream.shutdown(std::net::Shutdown::Both);
    eprintln!("admin helper: picoclaw install started");
    let install = download::RuntimeInstall {
        curl: Path::new(download::CURL),
        tar: Path::new("/bin/tar"),
        url: PICOCLAW_RELEASE_URL,
        sha256: PICOCLAW_RELEASE_SHA256,
        bytes: PICOCLAW_RELEASE_BYTES,
        directory: Path::new(PICOCLAW_RUNTIME_DIR),
        status: install_status,
    };
    match install.run(picoclaw_enabled) {
        Ok(()) => eprintln!("admin helper: picoclaw installed"),
        Err(error) => eprintln!("admin helper: picoclaw install failed: {error}"),
    }
    Ok(())
}

fn remove_picoclaw(stream: &mut UnixStream) -> io::Result<()> {
    let status = Path::new(DOWNLOAD_STATUS);
    let Some(_lock) = download::DownloadLock::acquire(&status.with_file_name("download.lock"))?
    else {
        return refuse(stream, "a download is in progress");
    };
    match std::fs::remove_file(PICOCLAW_BINARY) {
        Ok(()) => eprintln!("admin helper: picoclaw removed"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return refuse(stream, &error.to_string()),
    }
    let _ = std::fs::remove_file(PICOCLAW_INSTALL_STATUS);
    write_line(stream, &AdminReply::Done)
}

/// The PicoClaw configuration helper, as the runtime's own user. It only
/// touches that user's files and starts nothing, so the web server's switch
/// check covers it; the runtime start and its KVM access are re-checked.
fn serve_picoclaw(stream: &mut UnixStream) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let request: AdminRequest = read_line(stream)?;
    let store =
        picoclaw::Picoclaw::new(PICOCLAW_STATE, PICOCLAW_BINARY, picoclaw::PROFILE_DIRECTORY);
    let reply = match request {
        AdminRequest::PicoclawState {} => store.facts().map(|facts| AdminReply::Picoclaw { facts }),
        AdminRequest::PicoclawPrepare {
            internal_token,
            mcp_url,
        } => store
            .prepare(&internal_token, &mcp_url)
            .map(|()| AdminReply::Done),
        AdminRequest::PicoclawModel {
            model,
            api_base,
            api_key,
        } => store.model(&model, &api_base, &api_key).map(|model_name| {
            eprintln!("admin helper: picoclaw model set to {model_name}");
            AdminReply::PicoclawModelSaved { model_name }
        }),
        AdminRequest::PicoclawProfile { profile } => {
            store.profile(&profile).map(|()| AdminReply::Done)
        }
        AdminRequest::PicoclawSessions { offset, limit } => store
            .sessions(offset, limit)
            .map(|items| AdminReply::PicoclawSessions { items }),
        AdminRequest::PicoclawSession { id } => store
            .session(&id)
            .map(|detail| AdminReply::PicoclawSession { detail }),
        AdminRequest::PicoclawSessionDelete { id } => {
            store.delete_session(&id).map(|()| AdminReply::Done)
        }
        AdminRequest::PicoclawReset {} => store.reset().map(|()| {
            eprintln!("admin helper: picoclaw configuration removed");
            AdminReply::Done
        }),
        _ => Err(io::Error::other("not a picoclaw request")),
    };
    let reply = reply.unwrap_or_else(|error| AdminReply::Refused {
        reason: error.to_string(),
    });
    write_reply(stream, &reply)
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
