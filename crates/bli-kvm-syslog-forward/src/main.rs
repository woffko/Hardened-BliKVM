//! Runs the forwarder under its unit: systemd passes `syslog.socket` as
//! descriptor 3 and the settings as the environment, and gives it its state
//! directory on the card (docs/baselines/work-plan-20261117-system-log.md, L2).

use std::io::{Read, Seek, SeekFrom};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::os::fd::FromRawFd;
use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use bli_kvm_syslog_forward::{
    Message, Settings, Store, boot_id, hostname, open_kmsg, parse_forwarded, parse_kmsg,
    remote_line, sent,
};

/// How often the remote host's name is resolved again.
const RESOLVE_EVERY: Duration = Duration::from_secs(300);

fn main() -> ExitCode {
    let settings = match Settings::from_values(
        std::env::var("HARDENED_BLIKVM_SYSLOG_REMOTE")
            .ok()
            .as_deref(),
        std::env::var("HARDENED_BLIKVM_SYSLOG_PRIORITY")
            .ok()
            .as_deref(),
        std::env::var("HARDENED_BLIKVM_SYSLOG_KERNEL")
            .ok()
            .as_deref(),
    ) {
        Ok(settings) => settings,
        Err(error) => {
            // The errors are still kept on the card; nothing is sent.
            eprintln!("syslog forward: invalid settings ({error:?}); nothing is sent");
            Settings::default()
        }
    };
    let Some(state) = std::env::var_os("STATE_DIRECTORY").map(PathBuf::from) else {
        eprintln!("syslog forward: no state directory");
        return ExitCode::FAILURE;
    };
    let mut store = Store::new(&state);
    let new_boot = match boot_id()
        .and_then(|id| store.mark_boot(&id, &state.join("boot"), SystemTime::now()))
    {
        Ok(new_boot) => new_boot,
        Err(error) => {
            eprintln!("syslog forward: the boot could not be marked: {error}");
            false
        }
    };

    let (sender, receiver) = mpsc::sync_channel::<Message>(1024);
    if let Some(socket) = systemd_socket() {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let mut buffer = vec![0_u8; 8192];
            loop {
                match socket.recv(&mut buffer) {
                    Ok(length) => {
                        // The kernel's lines come from /dev/kmsg.
                        if let Some(message) = parse_forwarded(&buffer[..length])
                            && !message.is_kernel()
                            && sender.send(message).is_err()
                        {
                            return;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) => {
                        eprintln!("syslog forward: the socket failed: {error}");
                        std::process::exit(1);
                    }
                }
            }
        });
    } else {
        eprintln!("syslog forward: no socket from systemd; the kernel's lines only");
    }
    match open_kmsg() {
        Ok(mut kmsg) => {
            // A boot's own start is read once; a restart within it reads on.
            if !new_boot {
                let _ = kmsg.seek(SeekFrom::End(0));
            }
            let sender = sender.clone();
            std::thread::spawn(move || {
                let mut buffer = vec![0_u8; 8192];
                loop {
                    match kmsg.read(&mut buffer) {
                        Ok(0) => return,
                        Ok(length) => {
                            if let Some(message) = parse_kmsg(&buffer[..length])
                                && sender.send(message).is_err()
                            {
                                return;
                            }
                        }
                        // Records overwritten before they were read.
                        Err(error) if error.raw_os_error() == Some(32) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(error) => {
                            eprintln!("syslog forward: /dev/kmsg failed: {error}");
                            return;
                        }
                    }
                }
            });
        }
        Err(error) => eprintln!("syslog forward: /dev/kmsg could not be opened: {error}"),
    }
    drop(sender);

    let host = hostname();
    let mut remote = Remote::new(settings.remote.clone());
    for message in receiver {
        let now = SystemTime::now();
        if let Err(error) = store.keep(&message, now) {
            eprintln!("syslog forward: the card file failed: {error}");
        }
        if sent(&settings, &message) {
            remote.send(&remote_line(&message, &host, now));
        }
    }
    ExitCode::SUCCESS
}

/// The datagram socket systemd passed, when it passed exactly one to us.
fn systemd_socket() -> Option<UnixDatagram> {
    let pid: u32 = std::env::var("LISTEN_PID").ok()?.parse().ok()?;
    let count: u32 = std::env::var("LISTEN_FDS").ok()?.parse().ok()?;
    if pid != std::process::id() || count != 1 {
        return None;
    }
    // SAFETY: systemd passed exactly one descriptor, 3 (SD_LISTEN_FDS_START),
    // to this process, as LISTEN_PID and LISTEN_FDS say; nothing else in the
    // process owns it, and it is taken once.
    Some(unsafe { UnixDatagram::from_raw_fd(3) })
}

/// The remote syslog host, resolved again now and then.
struct Remote {
    target: Option<(String, u16)>,
    socket: Option<UdpSocket>,
    address: Option<SocketAddr>,
    resolved: Option<Instant>,
}

impl Remote {
    fn new(target: Option<(String, u16)>) -> Self {
        Self {
            target,
            socket: None,
            address: None,
            resolved: None,
        }
    }

    fn send(&mut self, line: &str) {
        let Some((host, port)) = self.target.as_ref() else {
            return;
        };
        if self.resolved.is_none_or(|at| at.elapsed() >= RESOLVE_EVERY) {
            self.resolved = Some(Instant::now());
            let bare = host.trim_start_matches('[').trim_end_matches(']');
            self.address = (bare, *port)
                .to_socket_addrs()
                .ok()
                .and_then(|mut addresses| addresses.next());
            self.socket = self.address.and_then(|address| {
                let local = if address.is_ipv4() {
                    "0.0.0.0:0"
                } else {
                    "[::]:0"
                };
                UdpSocket::bind(local).ok()
            });
        }
        if let (Some(socket), Some(address)) = (self.socket.as_ref(), self.address) {
            let _ = socket.send_to(line.as_bytes(), address);
        }
    }
}
