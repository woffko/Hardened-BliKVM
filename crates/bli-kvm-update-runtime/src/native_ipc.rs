//! Bounded, peer-bound transport for native readiness line protocols.
//! Callers must supply fixed read-only requests and validate protocol responses.

use socket2::{Domain, SockAddr, Socket, Type};
use std::{
    io::{self, Read, Write},
    net::Shutdown,
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::Path,
    thread,
    time::{Duration, Instant},
};

const MAX_LINE: usize = 8192;

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "native IPC deadline expired"))
}

fn pause(deadline: Instant) -> io::Result<()> {
    thread::sleep(remaining(deadline)?.min(Duration::from_millis(2)));
    Ok(())
}

fn check_peer(stream: &UnixStream, expected_pid: u32) -> io::Result<()> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: stream owns the live socket; both output pointers refer to fully
    // initialized objects of the exact Linux SO_PEERCRED ABI sizes. The kernel
    // cannot write more than the supplied buffer length, which is checked below.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of::<libc::ucred>()
        || expected_pid == 0
        || expected_pid > i32::MAX as u32
        || credentials.pid != expected_pid as i32
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "native IPC peer identity mismatch",
        ));
    }
    Ok(())
}

/// Send exactly one bounded line to an already identity-verified process.
/// Peer PID is checked before writing any request bytes. The caller must also
/// re-observe process start-time/executable identity after the probe sequence.
/// Socket/protocol errors do not retry connection or replay requests.
pub fn exchange_line(
    path: &Path,
    expected_pid: u32,
    request: &[u8],
    deadline: Instant,
) -> io::Result<Vec<u8>> {
    remaining(deadline)?;
    if !path.is_absolute()
        || request.is_empty()
        || request.len() > MAX_LINE
        || request.last() != Some(&b'\n')
        || request[..request.len() - 1].contains(&b'\n')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid native IPC request",
        ));
    }
    let mut stream = connect_and_write(path, expected_pid, request, deadline, true)?;
    read_line(&mut stream, deadline)
}

fn connect_and_write(
    path: &Path,
    expected_pid: u32,
    request: &[u8],
    deadline: Instant,
    half_close: bool,
) -> io::Result<UnixStream> {
    remaining(deadline)?;
    if !path.is_absolute() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let address = SockAddr::unix(path)?;
    let socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
    socket.set_nonblocking(true)?;
    // A full Unix accept queue fails immediately rather than blocking connect.
    // An in-progress/error result is not interpreted as successful admission.
    socket.connect(&address)?;
    let mut stream = UnixStream::from(std::os::fd::OwnedFd::from(socket));
    check_peer(&stream, expected_pid)?;
    let mut unsent = request;
    while !unsent.is_empty() {
        remaining(deadline)?;
        match stream.write(unsent) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(count) => unsent = &unsent[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => pause(deadline)?,
            Err(error) => return Err(error),
        }
    }
    if half_close {
        stream.shutdown(Shutdown::Write)?;
    }
    Ok(stream)
}

fn read_line(stream: &mut UnixStream, deadline: Instant) -> io::Result<Vec<u8>> {
    let mut response = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        remaining(deadline)?;
        match stream.read(&mut buffer) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete native IPC line",
                ));
            }
            Ok(count) => {
                if response.len() + count > MAX_LINE {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "native IPC response too large",
                    ));
                }
                response.extend_from_slice(&buffer[..count]);
                if let Some(end) = response.iter().position(|byte| *byte == b'\n') {
                    if end + 1 != response.len() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "multiple native IPC response lines",
                        ));
                    }
                    remaining(deadline)?;
                    return Ok(response);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => pause(deadline)?,
            Err(error) => return Err(error),
        }
    }
}

/// Exchange one length-prefixed control-protocol payload. The protocol crate
/// supplies the frame encoder and size limit; returned JSON must still pass its
/// typed decoder and request/operation correlation checks at the probe boundary.
pub fn exchange_control_frame(
    path: &Path,
    expected_pid: u32,
    payload: &[u8],
    deadline: Instant,
) -> io::Result<Vec<u8>> {
    use bli_kvm_control_protocol::{MAX_CONTROL_MESSAGE_BYTES, encode_frame};
    remaining(deadline)?;
    if payload.is_empty() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let frame = encode_frame(payload).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut stream = connect_and_write(path, expected_pid, &frame, deadline, true)?;
    let mut prefix = [0_u8; 4];
    read_exact_before(&mut stream, &mut prefix, deadline)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_CONTROL_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "native control frame length rejected",
        ));
    }
    let mut response = vec![0_u8; length];
    read_exact_before(&mut stream, &mut response, deadline)?;
    remaining(deadline)?;
    Ok(response)
}

pub(crate) fn read_exact_before(
    stream: &mut UnixStream,
    mut bytes: &mut [u8],
    deadline: Instant,
) -> io::Result<()> {
    while !bytes.is_empty() {
        remaining(deadline)?;
        match stream.read(bytes) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(count) => bytes = &mut bytes[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => pause(deadline)?,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(crate) fn open_mjpeg_probe(path: &Path, pid: u32, deadline: Instant) -> io::Result<UnixStream> {
    connect_and_write(
        path,
        pid,
        b"GET /mjpeg HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        deadline,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    fn fixture(
        reply: impl FnOnce(UnixStream) + Send + 'static,
    ) -> (
        tempfile::TempDir,
        std::path::PathBuf,
        thread::JoinHandle<()>,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("probe.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let worker = thread::spawn(move || reply(listener.accept().unwrap().0));
        (directory, path, worker)
    }

    #[test]
    fn exact_peer_exchanges_one_line_and_wrong_peer_receives_no_request() {
        for correct in [true, false] {
            let (_directory, path, worker) = fixture(move |mut stream| {
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                stream.read_to_end(&mut request).unwrap();
                if correct {
                    assert_eq!(request, b"status\n");
                    stream.write_all(b"ready\n").unwrap();
                } else {
                    assert!(request.is_empty());
                }
            });
            let result = exchange_line(
                &path,
                std::process::id() + u32::from(!correct),
                b"status\n",
                Instant::now() + Duration::from_secs(1),
            );
            if correct {
                assert_eq!(result.unwrap(), b"ready\n");
            } else {
                assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
            }
            worker.join().unwrap();
        }
    }

    #[test]
    fn oversized_and_truncated_lines_fail_closed() {
        for (response, expected) in [
            (vec![b'x'; MAX_LINE + 1], io::ErrorKind::InvalidData),
            (b"unfinished".to_vec(), io::ErrorKind::UnexpectedEof),
        ] {
            let (_directory, path, worker) = fixture(move |mut stream| {
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream.read_to_end(&mut Vec::new()).unwrap();
                let _ = stream.write_all(&response);
            });
            assert_eq!(
                exchange_line(
                    &path,
                    std::process::id(),
                    b"status\n",
                    Instant::now() + Duration::from_secs(1)
                )
                .unwrap_err()
                .kind(),
                expected
            );
            worker.join().unwrap();
        }
    }

    #[test]
    fn trickled_response_cannot_renew_deadline() {
        let (_directory, path, worker) = fixture(|mut stream| {
            for _ in 0..100 {
                if stream.write_all(b"x").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        let start = Instant::now();
        let error = exchange_line(
            &path,
            std::process::id(),
            b"status\n",
            start + Duration::from_millis(50),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(1));
        worker.join().unwrap();
    }

    #[test]
    fn full_accept_queue_does_not_block_connect() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("full.sock");
        let address = SockAddr::unix(&path).unwrap();
        let listener = Socket::new(Domain::UNIX, Type::STREAM, None).unwrap();
        listener.bind(&address).unwrap();
        listener.listen(0).unwrap();
        // Linux permits one queued connection with backlog zero. Never accept it.
        let queued = Socket::new(Domain::UNIX, Type::STREAM, None).unwrap();
        queued.set_nonblocking(true).unwrap();
        queued.connect(&address).unwrap();
        let start = Instant::now();
        let error = exchange_line(
            &path,
            std::process::id(),
            b"status\n",
            // A blocking connect would run into this deadline (TimedOut);
            // it is long enough that a loaded host still reaches connect.
            start + Duration::from_millis(900),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(start.elapsed() < Duration::from_millis(900));
    }

    #[test]
    fn control_frames_use_the_real_protocol_and_bound_the_prefix() {
        use bli_kvm_control_protocol::{decode_frame, encode_frame};
        for (reply, expected) in [
            (encode_frame(b"{}\n").unwrap(), None),
            (
                4097_u32.to_be_bytes().to_vec(),
                Some(io::ErrorKind::InvalidData),
            ),
            (
                0_u32.to_be_bytes().to_vec(),
                Some(io::ErrorKind::InvalidData),
            ),
            (vec![0, 0], Some(io::ErrorKind::UnexpectedEof)),
            (vec![0, 0, 0, 2, b'{'], Some(io::ErrorKind::UnexpectedEof)),
        ] {
            let (_directory, path, worker) = fixture(move |mut stream| {
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                stream.read_to_end(&mut request).unwrap();
                assert_eq!(decode_frame(&request).unwrap(), b"{}");
                stream.write_all(&reply).unwrap();
            });
            let result = exchange_control_frame(
                &path,
                std::process::id(),
                b"{}",
                Instant::now() + Duration::from_secs(1),
            );
            if let Some(expected) = expected {
                assert_eq!(result.unwrap_err().kind(), expected);
            } else {
                assert_eq!(result.unwrap(), b"{}\n");
            }
            worker.join().unwrap();
        }
    }

    #[test]
    fn control_payload_cannot_renew_the_prefix_deadline() {
        let (_directory, path, worker) = fixture(|mut stream| {
            // Under host load the client may time out and leave before this
            // first write: a peer that has gone is not a fixture failure.
            if stream.write_all(&100_u32.to_be_bytes()).is_err() {
                return;
            }
            for _ in 0..100 {
                if stream.write_all(b"x").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        assert_eq!(
            exchange_control_frame(
                &path,
                std::process::id(),
                b"{}",
                Instant::now() + Duration::from_millis(50)
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::TimedOut
        );
        worker.join().unwrap();
    }
}
