//! Newest-frame relay for the MJPEG stream.
//!
//! The capture passes MJPEG through at up to 60 frames a second and
//! 150-260 KB a frame. A viewer on a slower link used to receive every frame,
//! late: the frames waited in the socket buffers between the capture and the
//! browser, and the picture fell seconds behind. A task now reads the
//! upstream continuously and keeps only the newest complete part; the
//! response sends a part only when the connection can take one, so a slow
//! viewer skips frames instead of falling behind.

use std::convert::Infallible;

use bytes::{Buf, Bytes, BytesMut};
use http_body::{Body as HttpBody, Frame};
use http_body_util::{BodyExt, StreamBody};
use tokio::sync::watch;

/// A part's headers end within this many bytes, or the stream is resynced.
const MAX_HEADER_BYTES: usize = 4096;
/// A larger part (or one that never ends) is dropped and the stream resynced.
const MAX_PART_BYTES: usize = 16 * 1024 * 1024;

/// Splits a `multipart/x-mixed-replace` stream into whole parts.
pub(crate) struct PartParser {
    /// `--` and the boundary.
    delimiter: Vec<u8>,
    buffer: BytesMut,
}

impl PartParser {
    pub(crate) fn new(boundary: &str) -> Self {
        let mut delimiter = b"--".to_vec();
        delimiter.extend_from_slice(boundary.as_bytes());
        Self {
            delimiter,
            buffer: BytesMut::new(),
        }
    }

    /// Takes the next bytes of the stream; returns the newest part that is now
    /// complete (boundary line, headers, body and a closing CRLF), if any.
    /// Older complete parts in the same bytes are dropped.
    pub(crate) fn push(&mut self, data: &[u8]) -> Option<Bytes> {
        self.buffer.extend_from_slice(data);
        let mut newest = None;
        while let Some(part) = self.next_part() {
            newest = Some(part);
        }
        if self.buffer.len() > MAX_PART_BYTES {
            // A part without an end: start over at the next boundary.
            self.buffer.clear();
        }
        newest
    }

    fn next_part(&mut self) -> Option<Bytes> {
        loop {
            while matches!(self.buffer.first(), Some(b'\r' | b'\n')) {
                self.buffer.advance(1);
            }
            if !self.buffer.starts_with(&self.delimiter) {
                // Resync on the next boundary; keep a tail that may start one.
                match find(&self.buffer, &self.delimiter) {
                    Some(start) => self.buffer.advance(start),
                    None => {
                        let keep = self.delimiter.len().saturating_sub(1);
                        let drop = self.buffer.len().saturating_sub(keep);
                        self.buffer.advance(drop);
                        return None;
                    }
                }
            }
            let Some(header_end) = find(&self.buffer, b"\r\n\r\n") else {
                if self.buffer.len() > MAX_HEADER_BYTES {
                    self.buffer.advance(self.delimiter.len());
                    continue;
                }
                return None;
            };
            let body_start = header_end + 4;
            let end = match content_length(&self.buffer[..header_end]) {
                Some(length) if length <= MAX_PART_BYTES => body_start + length,
                Some(_) => {
                    self.buffer.advance(self.delimiter.len());
                    continue;
                }
                // No length: the part ends where the next one starts.
                None => {
                    let mut next = b"\r\n".to_vec();
                    next.extend_from_slice(&self.delimiter);
                    match find(&self.buffer[body_start..], &next) {
                        Some(offset) => body_start + offset,
                        None => return None,
                    }
                }
            };
            if self.buffer.len() < end {
                return None;
            }
            let mut part = self.buffer.split_to(end);
            part.extend_from_slice(b"\r\n");
            return Some(part.freeze());
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn content_length(headers: &[u8]) -> Option<usize> {
    std::str::from_utf8(headers)
        .ok()?
        .split("\r\n")
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
}

/// The boundary of a `multipart/x-mixed-replace` content type.
pub(crate) fn boundary(content_type: &str) -> Option<&str> {
    content_type.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        let value = value.trim().trim_matches('"');
        (name.trim().eq_ignore_ascii_case("boundary") && !value.is_empty()).then_some(value)
    })
}

/// Relays `upstream` so that a slow reader gets the newest part, not a queue.
pub(crate) fn newest_parts<B>(
    upstream: B,
    boundary: &str,
) -> impl HttpBody<Data = Bytes, Error = Infallible> + Send + 'static
where
    B: HttpBody<Data = Bytes> + Send + 'static,
{
    let (sender, receiver) = watch::channel::<Option<Bytes>>(None);
    let mut parser = PartParser::new(boundary);
    tokio::spawn(async move {
        let mut upstream = Box::pin(upstream);
        loop {
            tokio::select! {
                // The viewer left: stop reading, which closes the upstream.
                () = sender.closed() => break,
                frame = upstream.frame() => match frame {
                    Some(Ok(frame)) => {
                        if let Ok(data) = frame.into_data()
                            && let Some(part) = parser.push(&data)
                        {
                            sender.send_replace(Some(part));
                        }
                    }
                    _ => break,
                },
            }
        }
    });
    StreamBody::new(futures_util::stream::unfold(
        receiver,
        |mut receiver| async move {
            receiver.changed().await.ok()?;
            let part = receiver.borrow_and_update().clone()?;
            Some((Ok(Frame::data(part)), receiver))
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(n: u8, length: usize) -> Vec<u8> {
        let mut bytes =
            format!("--b\r\nContent-Type: image/jpeg\r\nContent-Length: {length}\r\n\r\n")
                .into_bytes();
        bytes.extend(std::iter::repeat_n(n, length));
        bytes.extend_from_slice(b"\r\n");
        bytes
    }

    #[test]
    fn the_newest_complete_part_wins() {
        let mut parser = PartParser::new("b");
        let mut stream = part(1, 10);
        stream.extend(part(2, 20));
        stream.extend(&part(3, 30)[..15]);
        let newest = parser.push(&stream).unwrap();
        assert_eq!(&newest[..], &part(2, 20)[..]);
        // The third part completes with the next bytes.
        assert_eq!(
            &parser.push(&part(3, 30)[15..]).unwrap()[..],
            &part(3, 30)[..]
        );
        assert!(parser.push(b"").is_none());
    }

    #[test]
    fn garbage_is_skipped_and_parts_without_a_length_end_at_the_next_boundary() {
        let mut parser = PartParser::new("b");
        let mut stream = b"noise before the stream".to_vec();
        stream.extend_from_slice(b"--b\r\nContent-Type: image/jpeg\r\n\r\nJPEG\r\n");
        assert!(parser.push(&stream).is_none(), "its end is not known yet");
        let next = part(4, 4);
        assert_eq!(
            &parser.push(&next[..10]).unwrap()[..],
            b"--b\r\nContent-Type: image/jpeg\r\n\r\nJPEG\r\n"
        );
        assert_eq!(&parser.push(&next[10..]).unwrap()[..], &next[..]);
    }

    #[test]
    fn the_boundary_comes_from_the_content_type() {
        assert_eq!(
            boundary("multipart/x-mixed-replace;boundary=boundarydonotcross"),
            Some("boundarydonotcross")
        );
        assert_eq!(
            boundary("multipart/x-mixed-replace; boundary=\"frame\""),
            Some("frame")
        );
        assert_eq!(boundary("multipart/x-mixed-replace"), None);
    }

    #[tokio::test]
    async fn a_slow_reader_skips_to_the_newest_frame() {
        let (sender, receiver) = tokio::sync::mpsc::channel::<Bytes>(16);
        let upstream = StreamBody::new(futures_util::stream::unfold(
            receiver,
            |mut receiver| async move {
                let data = receiver.recv().await?;
                Some((Ok::<_, Infallible>(Frame::data(data)), receiver))
            },
        ));
        let mut body = Box::pin(newest_parts(upstream, "b"));
        sender.send(Bytes::from(part(1, 8))).await.unwrap();
        let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
        assert_eq!(&first[..], &part(1, 8)[..]);
        // The reader is busy while three frames arrive; it gets the last.
        for n in 2..=4 {
            sender.send(Bytes::from(part(n, 8))).await.unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let next = body.frame().await.unwrap().unwrap().into_data().unwrap();
        assert_eq!(&next[..], &part(4, 8)[..]);
        // The upstream ending ends the response.
        drop(sender);
        assert!(body.frame().await.is_none());
    }
}
