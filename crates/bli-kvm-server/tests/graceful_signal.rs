//! Real server process signals with temporary state and loopback HTTP only.

use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn healthy(address: SocketAddr) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(100)) else {
        return false;
    };
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    if stream
        .write_all(b"GET /api/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut output = Vec::new();
    let mut buffer = [0_u8; 1024];
    let deadline = Instant::now() + Duration::from_millis(200);
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return false;
        };
        if remaining.is_zero() || stream.set_read_timeout(Some(remaining)).is_err() {
            return false;
        }
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) if output.len() + count <= 8192 => output.extend_from_slice(&buffer[..count]),
            _ => return false,
        }
    }
    let Ok(output) = std::str::from_utf8(&output) else {
        return false;
    };
    output.starts_with("HTTP/1.1 200") && output.contains("\"status\":\"ok\"")
}

fn stalled_request(address: SocketAddr) -> TcpStream {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(1)).unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    stream.write_all(b"POST /api/auth/login HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 100\r\nExpect: 100-continue\r\n\r\n").unwrap();
    let mut response = [0_u8; 25];
    let mut offset = 0;
    let deadline = Instant::now() + Duration::from_secs(1);
    while offset < response.len() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("100 Continue deadline");
        stream.set_read_timeout(Some(remaining)).unwrap();
        let count = stream.read(&mut response[offset..]).unwrap();
        assert_ne!(count, 0, "server closed before requesting the body");
        offset += count;
    }
    assert_eq!(&response, b"HTTP/1.1 100 Continue\r\n\r\n");
    stream
}

fn signal_gate(signal: libc::c_int, stall: bool) {
    let directory = tempfile::tempdir().unwrap();
    let web = directory.path().join("web");
    fs::create_dir(&web).unwrap();
    fs::write(web.join("index.html"), "signal fixture").unwrap();
    let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let mut server = Server(
        Command::new(env!("CARGO_BIN_EXE_bli-kvm-server"))
            .env_clear()
            .env("BLI_KVM_BIND", address.to_string())
            .env("BLI_KVM_WEB_ROOT", &web)
            .env("BLI_KVM_STATE_DIR", directory.path().join("state"))
            .env(
                "BLI_KVM_BROKER_SOCKET",
                directory.path().join("absent-broker.sock"),
            )
            .env(
                "BLI_KVM_VIDEO_SOCKET",
                directory.path().join("absent-video.sock"),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "server exited before readiness"
        );
        if healthy(address) {
            break;
        }
        assert!(Instant::now() < deadline, "server readiness deadline");
        thread::sleep(Duration::from_millis(10));
    }
    // Keep the admitted request open throughout the signal/exit observation.
    let _request = stall.then(|| stalled_request(address));
    let pid = libc::pid_t::try_from(server.0.id()).unwrap();
    // SAFETY: this is the unreaped child owned by this test; signal is a fixed
    // SIGTERM or SIGINT constant. No host service or device is addressed.
    assert_eq!(unsafe { libc::kill(pid, signal) }, 0);
    let started = Instant::now();
    let deadline = started + Duration::from_secs(if stall { 23 } else { 3 });
    loop {
        if let Some(status) = server.0.try_wait().unwrap() {
            if stall {
                assert_eq!(status.code(), Some(1), "deadline must be an error exit");
                assert!(
                    started.elapsed() >= Duration::from_secs(19),
                    "request was not drained to the configured deadline"
                );
            } else {
                assert!(status.success(), "server did not exit cleanly: {status}");
            }
            break;
        }
        assert!(Instant::now() < deadline, "server signal drain deadline");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn sigterm_exits_normally_after_http_readiness() {
    signal_gate(libc::SIGTERM, false);
}

#[test]
fn sigint_exits_normally_after_http_readiness() {
    signal_gate(libc::SIGINT, false);
}

#[test]
fn stalled_request_cannot_produce_a_clean_signal_exit() {
    signal_gate(libc::SIGTERM, true);
}
