//! Bounded reader for uStreamer's `multipart/x-mixed-replace` MJPEG stream.
//!
//! Only the framing uStreamer emits is accepted: an HTTP/1.x 200 response
//! without transfer or content encoding, and parts that each carry an
//! explicit Content-Length. Anything else ends the stream with an error.

use std::io::{self, BufRead, BufReader, Read};

pub const MAX_HEADER_BYTES: usize = 8 * 1024;
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub struct MjpegReader<R> {
    reader: BufReader<R>,
    boundary: Vec<u8>,
}

fn read_line(reader: &mut impl BufRead, budget: &mut usize) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    let limit = (*budget).min(MAX_HEADER_BYTES) as u64 + 1;
    reader.by_ref().take(limit).read_until(b'\n', &mut line)?;
    if line.is_empty() {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    if line.len() as u64 >= limit || !line.ends_with(b"\r\n") {
        return Err(invalid("MJPEG header line is too long or unterminated"));
    }
    *budget -= line.len();
    line.truncate(line.len() - 2);
    Ok(line)
}

fn headers(reader: &mut impl BufRead) -> io::Result<Vec<(String, String)>> {
    let mut budget = MAX_HEADER_BYTES;
    let mut fields = Vec::new();
    loop {
        let line = read_line(reader, &mut budget)?;
        if line.is_empty() {
            return Ok(fields);
        }
        let text = std::str::from_utf8(&line).map_err(|_| invalid("non-UTF-8 header"))?;
        let (name, value) = text
            .split_once(':')
            .ok_or_else(|| invalid("malformed header"))?;
        fields.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        if fields.len() > 64 {
            return Err(invalid("too many headers"));
        }
    }
}

impl<R: Read> MjpegReader<R> {
    /// Consume and validate the HTTP response head.
    pub fn new(stream: R) -> io::Result<Self> {
        let mut reader = BufReader::with_capacity(64 * 1024, stream);
        let mut budget = MAX_HEADER_BYTES;
        let status = read_line(&mut reader, &mut budget)?;
        if !(status.starts_with(b"HTTP/1.0 200") || status.starts_with(b"HTTP/1.1 200")) {
            return Err(invalid("uStreamer did not answer 200"));
        }
        let fields = headers(&mut reader)?;
        let field = |name: &str| {
            fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        if field("transfer-encoding").is_some() || field("content-encoding").is_some() {
            return Err(invalid("encoded MJPEG streams are not supported"));
        }
        let content_type = field("content-type").ok_or_else(|| invalid("missing content type"))?;
        let boundary = content_type
            .strip_prefix("multipart/x-mixed-replace")
            .and_then(|rest| {
                rest.split(';')
                    .find_map(|part| part.trim().strip_prefix("boundary="))
            })
            .map(|value| value.trim_matches('"'))
            .filter(|value| (1..=70).contains(&value.len()))
            .ok_or_else(|| invalid("unsupported content type"))?;
        Ok(Self {
            reader,
            boundary: format!("--{boundary}").into_bytes(),
        })
    }

    /// Read the next JPEG part into `frame`, replacing its contents.
    pub fn next_frame(&mut self, frame: &mut Vec<u8>) -> io::Result<()> {
        let mut budget = MAX_HEADER_BYTES;
        // Parts are separated by CRLF; tolerate the empty line after a body.
        let mut line = read_line(&mut self.reader, &mut budget)?;
        if line.is_empty() {
            line = read_line(&mut self.reader, &mut budget)?;
        }
        if line != self.boundary {
            return Err(invalid("missing multipart boundary"));
        }
        let fields = headers(&mut self.reader)?;
        let mut lengths = fields.iter().filter(|(key, _)| key == "content-length");
        let length = match (lengths.next(), lengths.next()) {
            (Some((_, value)), None) => value
                .parse::<usize>()
                .ok()
                .filter(|length| {
                    (4..=MAX_FRAME_BYTES).contains(length)
                        && value.bytes().all(|b| b.is_ascii_digit())
                })
                .ok_or_else(|| invalid("invalid part length"))?,
            _ => return Err(invalid("part needs exactly one Content-Length")),
        };
        frame.clear();
        frame.resize(length, 0);
        self.reader.read_exact(frame)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(parts: &[&[u8]]) -> Vec<u8> {
        let mut bytes = b"HTTP/1.0 200 OK\r\nContent-Type: multipart/x-mixed-replace;boundary=boundarydonotcross\r\nCache-Control: no-store\r\n\r\n".to_vec();
        for part in parts {
            bytes.extend_from_slice(b"--boundarydonotcross\r\nContent-Type: image/jpeg\r\n");
            bytes.extend_from_slice(
                format!("Content-Length: {}\r\nX-Timestamp: 1.0\r\n\r\n", part.len()).as_bytes(),
            );
            bytes.extend_from_slice(part);
            bytes.extend_from_slice(b"\r\n");
        }
        bytes
    }

    #[test]
    fn frames_are_read_in_order_until_eof() {
        let bytes = stream(&[b"\xff\xd8one\xff\xd9", b"\xff\xd8two!\xff\xd9"]);
        let mut reader = MjpegReader::new(bytes.as_slice()).unwrap();
        let mut frame = Vec::new();
        reader.next_frame(&mut frame).unwrap();
        assert_eq!(frame, b"\xff\xd8one\xff\xd9");
        reader.next_frame(&mut frame).unwrap();
        assert_eq!(frame, b"\xff\xd8two!\xff\xd9");
        assert_eq!(
            reader.next_frame(&mut frame).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn unexpected_framing_is_rejected() {
        for head in [
            &b"HTTP/1.1 404 Not Found\r\n\r\n"[..],
            b"HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: multipart/x-mixed-replace;boundary=b\r\n\r\n",
        ] {
            assert!(MjpegReader::new(head).is_err());
        }
        let mut long = b"HTTP/1.1 200 OK\r\nX: ".to_vec();
        long.extend(std::iter::repeat_n(b'a', MAX_HEADER_BYTES));
        assert!(MjpegReader::new(long.as_slice()).is_err());

        let head =
            b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=b\r\n\r\n";
        for part in [
            &b"--other\r\nContent-Length: 4\r\n\r\nabcd"[..],
            b"--b\r\n\r\nabcd",
            b"--b\r\nContent-Length: 4\r\nContent-Length: 4\r\n\r\nabcd",
            b"--b\r\nContent-Length: +4\r\n\r\nabcd",
            b"--b\r\nContent-Length: 99999999\r\n\r\nabcd",
            b"--b\r\nContent-Length: 8\r\n\r\nabcd",
        ] {
            let mut bytes = head.to_vec();
            bytes.extend_from_slice(part);
            let mut reader = MjpegReader::new(bytes.as_slice()).unwrap();
            assert!(reader.next_frame(&mut Vec::new()).is_err(), "{part:?}");
        }
    }
}
