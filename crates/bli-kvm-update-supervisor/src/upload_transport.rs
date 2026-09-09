//! Bounded upload reception, independent of slot-root locking and publication.
//!
//! The worker must authorize the kernel peer and acquire its upload admission
//! guard BEFORE calling this function. The sink factory must create a private
//! anonymous spool and check physical capacity, without opening the stager.
//! Only a successful return permits verification/staging. This is not an
//! authenticated worker entrypoint by itself.

use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

use bli_kvm_update_protocol::{
    MAX_UPDATE_MESSAGE_BYTES,
    upload::{UploadRequest, UploadRequestEnvelope, decode_request},
};

/// Limits apply independently: progress never extends the total deadline.
#[derive(Clone, Copy)]
pub struct ReceiveLimits {
    pub header: Duration,
    pub idle: Duration,
    pub total: Duration,
}

impl Default for ReceiveLimits {
    fn default() -> Self {
        Self {
            header: Duration::from_secs(2),
            idle: Duration::from_secs(5),
            total: Duration::from_secs(120),
        }
    }
}

struct DeadlineReader<'a> {
    stream: &'a mut UnixStream,
    deadline: Instant,
    idle: Duration,
}

impl Read for DeadlineReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            self.stream
                .set_read_timeout(Some(remaining.min(self.idle)))?;
            match self.stream.read(bytes) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => return result,
            }
        }
    }
}

/// Receive exactly one metadata frame, its declared body, and write-half EOF.
///
/// Memory usage is bounded independently of archive size. On any error the
/// owned sink is dropped; a worker must use an unlink-on-drop/anonymous file.
/// Sink writes must be local and bounded by the worker's process lifetime.
pub fn receive<S: Write>(
    stream: &mut UnixStream,
    limits: ReceiveLimits,
    create_sink: impl FnOnce(u64) -> io::Result<S>,
) -> io::Result<(UploadRequestEnvelope, S)> {
    let start = Instant::now();
    if limits.header.is_zero() || limits.idle.is_zero() || limits.total.is_zero() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let total_deadline = start
        .checked_add(limits.total)
        .ok_or(io::ErrorKind::InvalidInput)?;
    let mut reader = DeadlineReader {
        stream,
        deadline: start
            .checked_add(limits.header.min(limits.total))
            .ok_or(io::ErrorKind::InvalidInput)?,
        idle: limits.idle,
    };
    let mut prefix = [0; 4];
    reader.read_exact(&mut prefix)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_UPDATE_MESSAGE_BYTES {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut metadata = vec![0; length];
    reader.read_exact(&mut metadata)?;
    let request = decode_request(&metadata).map_err(|_| io::ErrorKind::InvalidData)?;
    let UploadRequest::Stage { archive_bytes } = request.request;
    let mut sink = create_sink(archive_bytes)?;
    reader.deadline = total_deadline;
    let mut remaining = archive_bytes;
    let mut buffer = [0; 64 * 1024];
    while remaining != 0 {
        let length = remaining.min(buffer.len() as u64) as usize;
        let count = reader.read(&mut buffer[..length])?;
        if count == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        sink.write_all(&buffer[..count])?;
        remaining -= count as u64;
    }
    if reader.read(&mut buffer[..1])? != 0 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    sink.flush()?;
    if Instant::now() >= total_deadline {
        return Err(io::ErrorKind::TimedOut.into());
    }
    Ok((request, sink))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_update_protocol::{encode_frame, upload::encode_request};
    use std::{net::Shutdown, thread};

    fn frame(size: u64) -> Vec<u8> {
        encode_frame(
            &encode_request(&UploadRequestEnvelope {
                schema_version: 1,
                request_id: 42,
                request: UploadRequest::Stage {
                    archive_bytes: size,
                },
            })
            .unwrap(),
        )
        .unwrap()
    }

    fn short_limits() -> ReceiveLimits {
        ReceiveLimits {
            header: Duration::from_millis(100),
            idle: Duration::from_millis(100),
            total: Duration::from_millis(200),
        }
    }

    #[test]
    fn exact_body_and_eof_return_owned_spool() {
        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        sender.write_all(&frame(3)).unwrap();
        sender.write_all(b"abc").unwrap();
        sender.shutdown(Shutdown::Write).unwrap();
        let (request, sink) = receive(&mut receiver, short_limits(), |size| {
            assert_eq!(size, 3);
            Ok(Vec::new())
        })
        .unwrap();
        assert_eq!(request.request_id, 42);
        assert_eq!(sink, b"abc");
    }

    #[test]
    fn short_and_surplus_bodies_fail() {
        for body in [b"ab".as_slice(), b"abcd".as_slice()] {
            let (mut sender, mut receiver) = UnixStream::pair().unwrap();
            sender.write_all(&frame(3)).unwrap();
            sender.write_all(body).unwrap();
            sender.shutdown(Shutdown::Write).unwrap();
            assert!(receive(&mut receiver, short_limits(), |_| Ok(Vec::new())).is_err());
        }
    }

    #[test]
    fn invalid_header_does_not_create_spool() {
        for bytes in [vec![0, 0, 0, 0], vec![255; 4], encode_frame(b"{}").unwrap()] {
            let (mut sender, mut receiver) = UnixStream::pair().unwrap();
            sender.write_all(&bytes).unwrap();
            sender.shutdown(Shutdown::Write).unwrap();
            assert!(
                receive::<Vec<u8>>(&mut receiver, short_limits(), |_| {
                    panic!("invalid metadata must not allocate a spool")
                })
                .is_err()
            );
        }
    }

    #[test]
    fn eof_is_required_even_after_exact_body() {
        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        sender.write_all(&frame(1)).unwrap();
        sender.write_all(b"a").unwrap();
        let error = receive(&mut receiver, short_limits(), |_| Ok(Vec::new())).unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ));
    }

    #[test]
    fn progress_does_not_extend_total_deadline() {
        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        sender.write_all(&frame(100)).unwrap();
        let writer = thread::spawn(move || {
            for _ in 0..20 {
                if sender.write_all(b"a").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
        });
        let start = Instant::now();
        let error = receive(&mut receiver, short_limits(), |_| Ok(Vec::new())).unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ));
        assert!(start.elapsed() < Duration::from_secs(2));
        drop(receiver);
        writer.join().unwrap();
    }

    #[test]
    fn stalled_header_never_creates_spool() {
        let (_sender, mut receiver) = UnixStream::pair().unwrap();
        assert!(
            receive::<Vec<u8>>(&mut receiver, short_limits(), |_| {
                panic!("incomplete header must not create a spool")
            })
            .is_err()
        );
    }

    #[test]
    fn partial_body_error_drops_owned_spool() {
        use std::{cell::Cell, rc::Rc};
        struct Sink(Rc<Cell<bool>>);
        impl Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        impl Drop for Sink {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        let dropped = Rc::new(Cell::new(false));
        let (mut sender, mut receiver) = UnixStream::pair().unwrap();
        sender.write_all(&frame(3)).unwrap();
        sender.write_all(b"a").unwrap();
        sender.shutdown(Shutdown::Write).unwrap();
        assert!(receive(&mut receiver, short_limits(), |_| Ok(Sink(dropped.clone()))).is_err());
        assert!(dropped.get());
    }
}
