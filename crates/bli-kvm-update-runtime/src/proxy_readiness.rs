//! First-part MJPEG transport evidence, not decoded-image/capture health.
use crate::native_ipc::{open_mjpeg_probe, read_exact_before};
use std::{collections::BTreeMap, io, os::unix::net::UnixStream, path::Path, time::Instant};

const MAX_HEADERS: usize = 8192;
const MAX_FRAME: usize = 4 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub struct MjpegTransportObservation {
    pub first_part_bytes: usize,
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "MJPEG transport response rejected",
    )
}

fn read_until(
    stream: &mut UnixStream,
    end: &[u8],
    limit: usize,
    deadline: Instant,
) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while bytes.len() < limit {
        let mut byte = [0_u8; 1];
        read_exact_before(stream, &mut byte, deadline)?;
        bytes.push(byte[0]);
        if bytes.ends_with(end) {
            return Ok(bytes);
        }
    }
    Err(invalid())
}

fn fields(headers: &[httparse::Header<'_>]) -> io::Result<BTreeMap<String, String>> {
    let mut fields = BTreeMap::new();
    for header in headers {
        let value = std::str::from_utf8(header.value)
            .map_err(|_| invalid())?
            .trim()
            .to_owned();
        if fields
            .insert(header.name.to_ascii_lowercase(), value)
            .is_some()
        {
            return Err(invalid());
        }
    }
    Ok(fields)
}

/// Uses the fixed proxy route and PID binding with one absolute deadline. The
/// supported stream framing is unencoded, non-chunked multipart with an explicit
/// per-part length. Other encodings fail closed until implemented and tested.
/// SOI/EOI delimiters only attest JPEG-shaped framing, not successful decoding.
pub fn probe_first_part(
    path: &Path,
    pid: u32,
    deadline: Instant,
) -> io::Result<MjpegTransportObservation> {
    let mut stream = open_mjpeg_probe(path, pid, deadline)?;
    let raw = read_until(&mut stream, b"\r\n\r\n", MAX_HEADERS, deadline)?;
    let mut storage = [httparse::EMPTY_HEADER; 32];
    let mut response = httparse::Response::new(&mut storage);
    if !matches!(response.parse(&raw), Ok(httparse::Status::Complete(size)) if size == raw.len())
        || response.code != Some(200)
    {
        return Err(invalid());
    }
    let headers = fields(response.headers)?;
    if headers.contains_key("transfer-encoding")
        || headers.contains_key("content-encoding")
        || headers.contains_key("content-length")
    {
        return Err(invalid());
    }
    let content_type = headers.get("content-type").ok_or_else(invalid)?;
    let (kind, parameter) = content_type.split_once(';').ok_or_else(invalid)?;
    if !kind
        .trim()
        .eq_ignore_ascii_case("multipart/x-mixed-replace")
    {
        return Err(invalid());
    }
    let (name, value) = parameter.trim().split_once('=').ok_or_else(invalid)?;
    if !name.eq_ignore_ascii_case("boundary") {
        return Err(invalid());
    }
    let value = value.trim();
    let boundary = if value.starts_with('"') {
        value
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .ok_or_else(invalid)?
    } else {
        value
    };
    if boundary.is_empty()
        || boundary.len() > 70
        || !boundary
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"'()+_,-./:=?".contains(&b))
    {
        return Err(invalid());
    }
    let mut line = read_until(&mut stream, b"\r\n", 76, deadline)?;
    if line == b"\r\n" {
        line = read_until(&mut stream, b"\r\n", 76, deadline)?;
    }
    if line != format!("--{boundary}\r\n").as_bytes() {
        return Err(invalid());
    }
    let raw = read_until(&mut stream, b"\r\n\r\n", MAX_HEADERS, deadline)?;
    let mut storage = [httparse::EMPTY_HEADER; 32];
    let headers = match httparse::parse_headers(&raw, &mut storage) {
        Ok(httparse::Status::Complete((size, headers))) if size == raw.len() => fields(headers)?,
        _ => return Err(invalid()),
    };
    if !headers
        .get("content-type")
        .is_some_and(|value| value.eq_ignore_ascii_case("image/jpeg"))
        || headers.contains_key("transfer-encoding")
        || headers.contains_key("content-encoding")
    {
        return Err(invalid());
    }
    let length = headers.get("content-length").ok_or_else(invalid)?;
    if !length.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let length: usize = length.parse().map_err(|_| invalid())?;
    if !(4..=MAX_FRAME).contains(&length) {
        return Err(invalid());
    }
    let mut frame = vec![0; length];
    read_exact_before(&mut stream, &mut frame, deadline)?;
    if !frame.starts_with(&[0xff, 0xd8]) || !frame.ends_with(&[0xff, 0xd9]) {
        return Err(invalid());
    }
    if Instant::now() >= deadline {
        return Err(io::ErrorKind::TimedOut.into());
    }
    Ok(MjpegTransportObservation {
        first_part_bytes: length,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        os::unix::net::UnixListener,
        thread,
        time::Duration,
    };
    const REQUEST: &[u8] = b"GET /mjpeg HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    const HEAD: &str = "HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\n\r\n--frame\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\n\r\n";

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn actual_proxy_keeps_request_open_and_releases_upstream_after_probe() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let directory = tempfile::tempdir().unwrap();
        let proxy_path = directory.path().join("proxy.sock");
        let upstream_path = directory.path().join("upstream.sock");
        let listener = tokio::net::UnixListener::bind(&upstream_path).unwrap();
        let proxy = tokio::spawn(bli_kvm_video_proxy::run(
            bli_kvm_video_proxy::VideoProxyConfig::new(&proxy_path, &upstream_path).unwrap(),
        ));
        for _ in 0..50 {
            if proxy_path.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let upstream = tokio::spawn(async move {
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                assert!(request.len() < 4096);
                let mut byte = [0_u8; 1];
                tokio::time::timeout(Duration::from_secs(1), stream.read_exact(&mut byte))
                    .await
                    .unwrap()
                    .unwrap();
                request.push(byte[0]);
            }
            assert!(request.starts_with(b"GET /stream HTTP/1.1\r\n"));
            let mut response = HEAD.as_bytes().to_vec();
            response.extend_from_slice(&[0xff, 0xd8, 0xff, 0xd9]);
            stream.write_all(&response).await.unwrap();
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(2), stream.read(&mut [0_u8; 1]))
                    .await
                    .unwrap()
                    .unwrap(),
                0
            );
        });
        let result = tokio::task::spawn_blocking(move || {
            probe_first_part(
                &proxy_path,
                std::process::id(),
                Instant::now() + Duration::from_secs(2),
            )
        })
        .await
        .unwrap();
        let upstream_result = upstream.await;
        proxy.abort();
        let _ = proxy.await;
        assert_eq!(result.unwrap().first_part_bytes, 4);
        upstream_result.unwrap();
    }

    #[test]
    fn complete_part_requires_valid_headers_size_and_open_request_half() {
        for case in 0..6 {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("proxy.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let worker = thread::spawn(move || {
                let mut stream = listener.accept().unwrap().0;
                stream
                    .set_read_timeout(Some(Duration::from_millis(30)))
                    .unwrap();
                let mut request = vec![0; REQUEST.len()];
                stream.read_exact(&mut request).unwrap();
                assert_eq!(request, REQUEST);
                let error = stream.read(&mut [0_u8; 1]).unwrap_err();
                assert!(matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ));
                let header = match case {
                    1 => HEAD.replace("200 OK", "503 Unavailable"),
                    2 => HEAD.replace("Content-Length: 4", "Content-Length: 4194305"),
                    3 => HEAD.replace("boundary=frame", "boundary=other"),
                    4 => HEAD.replace(
                        "Content-Length: 4",
                        "Content-Length: 4\r\nContent-Length: 4",
                    ),
                    _ => HEAD.into(),
                };
                let mut response = header.into_bytes();
                response.extend_from_slice(if case == 5 {
                    &[0xff, 0xd8]
                } else {
                    &[0xff, 0xd8, 0xff, 0xd9]
                });
                let _ = stream.write_all(&response);
            });
            let result = probe_first_part(
                &path,
                std::process::id(),
                Instant::now() + Duration::from_secs(1),
            );
            if case == 0 {
                assert_eq!(result.unwrap().first_part_bytes, 4);
            } else {
                assert!(result.is_err(), "case {case}");
            }
            worker.join().unwrap();
        }
    }
}
