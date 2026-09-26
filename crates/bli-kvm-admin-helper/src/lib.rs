//! Closed protocol between the web server and the root administration helper.
//!
//! One connection carries one request. The client sends a single JSON line;
//! the helper answers with a single JSON line and, for an interactive
//! operation, both sides then exchange length-prefixed frames. Every size is
//! bounded, unknown fields are refused, and nothing in a request names a
//! path, a command or a user.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// Socket-activated helper endpoint (one service instance per connection).
pub const ADMIN_SOCKET: &str = "/run/hardened-blikvm-admin/admin.sock";
pub const MAX_LINE_BYTES: usize = 4096;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

/// Terminal bytes in either direction.
pub const FRAME_DATA: u8 = 0;
/// Window size from the client: rows then columns, big-endian `u16`.
pub const FRAME_RESIZE: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdminRequest {
    /// A root login shell on a pseudo-terminal.
    Terminal { rows: u16, cols: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdminReply {
    Ready,
    Refused { reason: String },
}

/// Writes one JSON line.
pub fn write_line<T: Serialize>(writer: &mut impl Write, value: &T) -> io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    if line.len() >= MAX_LINE_BYTES {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()
}

/// Reads one JSON line of at most `MAX_LINE_BYTES`, one byte at a time so
/// that nothing after the newline is consumed.
pub fn read_line<T: for<'de> Deserialize<'de>>(reader: &mut impl Read) -> io::Result<T> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        if reader.read(&mut byte)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
        if line.len() >= MAX_LINE_BYTES {
            return Err(io::ErrorKind::InvalidData.into());
        }
    }
    serde_json::from_slice(&line).map_err(|_| io::ErrorKind::InvalidData.into())
}

pub fn encode_frame(kind: u8, payload: &[u8]) -> io::Result<Vec<u8>> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(kind);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

/// Reads one frame; `None` at a clean end of stream.
pub fn read_frame(reader: &mut impl Read) -> io::Result<Option<(u8, Vec<u8>)>> {
    let mut header = [0_u8; 5];
    match reader.read_exact(&mut header[..1]) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    reader.read_exact(&mut header[1..])?;
    let length = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if length > MAX_FRAME_BYTES || !matches!(header[0], FRAME_DATA | FRAME_RESIZE) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    Ok(Some((header[0], payload)))
}

pub fn encode_resize(rows: u16, cols: u16) -> [u8; 4] {
    let [r0, r1] = rows.to_be_bytes();
    let [c0, c1] = cols.to_be_bytes();
    [r0, r1, c0, c1]
}

/// A usable window size, or `None` for an empty or oversized one.
pub fn decode_resize(payload: &[u8]) -> Option<(u16, u16)> {
    let [r0, r1, c0, c1] = payload.try_into().ok()?;
    let rows = u16::from_be_bytes([r0, r1]);
    let cols = u16::from_be_bytes([c0, c1]);
    (1..=1000).contains(&rows).then_some(())?;
    (1..=1000).contains(&cols).then_some((rows, cols))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_frames_round_trip_within_bounds() {
        let mut buffer = Vec::new();
        write_line(&mut buffer, &AdminRequest::Terminal { rows: 24, cols: 80 }).unwrap();
        buffer.extend(encode_frame(FRAME_DATA, b"ls\r").unwrap());
        buffer.extend(encode_frame(FRAME_RESIZE, &encode_resize(40, 120)).unwrap());
        let mut reader = buffer.as_slice();
        assert_eq!(
            read_line::<AdminRequest>(&mut reader).unwrap(),
            AdminRequest::Terminal { rows: 24, cols: 80 }
        );
        assert_eq!(
            read_frame(&mut reader).unwrap(),
            Some((FRAME_DATA, b"ls\r".to_vec()))
        );
        let (kind, payload) = read_frame(&mut reader).unwrap().unwrap();
        assert_eq!(kind, FRAME_RESIZE);
        assert_eq!(decode_resize(&payload), Some((40, 120)));
        assert_eq!(read_frame(&mut reader).unwrap(), None);
    }

    #[test]
    fn malformed_requests_and_frames_are_refused() {
        for bad in [
            &b"{\"op\":\"terminal\",\"rows\":24,\"cols\":80,\"shell\":\"/bin/x\"}\n"[..],
            b"{\"op\":\"exec\"}\n",
            b"not json\n",
        ] {
            assert!(read_line::<AdminRequest>(&mut &bad[..]).is_err());
        }
        let long = vec![b'a'; MAX_LINE_BYTES + 1];
        assert!(read_line::<AdminRequest>(&mut long.as_slice()).is_err());
        assert!(encode_frame(FRAME_DATA, &vec![0; MAX_FRAME_BYTES + 1]).is_err());
        let mut oversized = vec![FRAME_DATA];
        oversized.extend(((MAX_FRAME_BYTES + 1) as u32).to_be_bytes());
        assert!(read_frame(&mut oversized.as_slice()).is_err());
        let unknown = [9_u8, 0, 0, 0, 0];
        assert!(read_frame(&mut unknown.as_slice()).is_err());
        assert_eq!(decode_resize(&encode_resize(0, 80)), None);
        assert_eq!(decode_resize(&[0, 1]), None);
    }
}
