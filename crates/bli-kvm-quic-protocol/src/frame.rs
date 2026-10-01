//! The header of an encoded frame. A key frame travels on a stream of its own
//! and a delta frame as datagrams; both begin with these 32 bytes, integers
//! most significant byte first:
//!
//! | Offset | Size | Field |
//! | --- | --- | --- |
//! | 0 | 1 | version (1) |
//! | 1 | 1 | kind: 1 key frame, 2 delta fragment |
//! | 2 | 1 | codec: 1 H.264, Annex B |
//! | 3 | 1 | reserved, zero |
//! | 4 | 2 | fragment index (zero for a key frame) |
//! | 6 | 2 | fragment count (one for a key frame) |
//! | 8 | 4 | stream id: new after an encoder restart or a change of size |
//! | 12 | 4 | frame id, rising by one within a stream |
//! | 16 | 4 | reference: the key frame this frame depends on |
//! | 20 | 4 | size of the complete frame |
//! | 24 | 8 | capture time, microseconds of the board's monotonic clock |
//!
//! On the encoder's frame socket a frame is one record: the same 32 bytes,
//! with the picture's width and height where the fragment fields are, and
//! then the whole frame.

use thiserror::Error;

pub const HEADER_BYTES: usize = 32;
/// A datagram never exceeds this, whatever the path would carry: it leaves
/// room for a VPN's own headers.
pub const MAX_DATAGRAM_BYTES: usize = 1200;
/// The smallest datagram size a frame is cut for.
pub const MIN_DATAGRAM_BYTES: usize = HEADER_BYTES + 64;
/// An encoded frame larger than this is refused.
pub const MAX_FRAME_BYTES: u32 = 4 * 1024 * 1024;
pub const CODEC_H264: u8 = 1;
const VERSION: u8 = 1;
const KIND_KEY: u8 = 1;
const KIND_DELTA: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Key,
    Delta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub kind: Kind,
    pub fragment_index: u16,
    pub fragment_count: u16,
    pub stream: u32,
    pub frame: u32,
    pub reference: u32,
    pub size: u32,
    pub capture_micros: u64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("fewer bytes than a frame header")]
    Short,
    #[error("unknown version, kind or codec, or a reserved byte that is not zero")]
    Unknown,
    #[error("a stream, frame or reference id that cannot be")]
    Identity,
    #[error("a frame size or fragment numbering out of range")]
    Size,
}

impl FrameHeader {
    /// The header of a whole key frame.
    pub fn key(stream: u32, frame: u32, size: u32, capture_micros: u64) -> Self {
        Self {
            kind: Kind::Key,
            fragment_index: 0,
            fragment_count: 1,
            stream,
            frame,
            reference: frame,
            size,
            capture_micros,
        }
    }

    fn validate(&self) -> Result<(), FrameError> {
        if self.stream == 0 || self.frame == 0 || self.reference == 0 {
            return Err(FrameError::Identity);
        }
        if self.size == 0 || self.size > MAX_FRAME_BYTES {
            return Err(FrameError::Size);
        }
        match self.kind {
            Kind::Key => {
                if self.reference != self.frame {
                    return Err(FrameError::Identity);
                }
                if self.fragment_index != 0 || self.fragment_count != 1 {
                    return Err(FrameError::Size);
                }
            }
            Kind::Delta => {
                // A delta frame depends on a key frame sent before it.
                if self.reference >= self.frame {
                    return Err(FrameError::Identity);
                }
                if self.fragment_count == 0
                    || self.fragment_index >= self.fragment_count
                    || u32::from(self.fragment_count) > self.size
                {
                    return Err(FrameError::Size);
                }
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<[u8; HEADER_BYTES], FrameError> {
        self.validate()?;
        let mut out = [0u8; HEADER_BYTES];
        out[0] = VERSION;
        out[1] = match self.kind {
            Kind::Key => KIND_KEY,
            Kind::Delta => KIND_DELTA,
        };
        out[2] = CODEC_H264;
        out[4..6].copy_from_slice(&self.fragment_index.to_be_bytes());
        out[6..8].copy_from_slice(&self.fragment_count.to_be_bytes());
        out[8..12].copy_from_slice(&self.stream.to_be_bytes());
        out[12..16].copy_from_slice(&self.frame.to_be_bytes());
        out[16..20].copy_from_slice(&self.reference.to_be_bytes());
        out[20..24].copy_from_slice(&self.size.to_be_bytes());
        out[24..32].copy_from_slice(&self.capture_micros.to_be_bytes());
        Ok(out)
    }

    /// The header at the start of `bytes`; what follows it is the payload.
    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError> {
        let header: &[u8; HEADER_BYTES] = bytes
            .get(..HEADER_BYTES)
            .and_then(|slice| slice.try_into().ok())
            .ok_or(FrameError::Short)?;
        if header[0] != VERSION || header[2] != CODEC_H264 || header[3] != 0 {
            return Err(FrameError::Unknown);
        }
        let kind = match header[1] {
            KIND_KEY => Kind::Key,
            KIND_DELTA => Kind::Delta,
            _ => return Err(FrameError::Unknown),
        };
        let u16_at = |at: usize| u16::from_be_bytes([header[at], header[at + 1]]);
        let u32_at = |at: usize| {
            u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
        };
        let mut capture = [0u8; 8];
        capture.copy_from_slice(&header[24..32]);
        let decoded = Self {
            kind,
            fragment_index: u16_at(4),
            fragment_count: u16_at(6),
            stream: u32_at(8),
            frame: u32_at(12),
            reference: u32_at(16),
            size: u32_at(20),
            capture_micros: u64::from_be_bytes(capture),
        };
        decoded.validate()?;
        Ok(decoded)
    }
}

/// The header of a record on the encoder's frame socket; the frame's bytes
/// follow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordHeader {
    pub kind: Kind,
    /// The size of the picture the stream decodes to.
    pub width: u16,
    pub height: u16,
    pub stream: u32,
    pub frame: u32,
    pub reference: u32,
    pub size: u32,
    pub capture_micros: u64,
}

impl RecordHeader {
    fn validate(&self) -> Result<(), FrameError> {
        if self.stream == 0 || self.frame == 0 || self.reference == 0 {
            return Err(FrameError::Identity);
        }
        if self.size == 0 || self.size > MAX_FRAME_BYTES || self.width == 0 || self.height == 0 {
            return Err(FrameError::Size);
        }
        match self.kind {
            Kind::Key if self.reference != self.frame => Err(FrameError::Identity),
            // A delta frame depends on a key frame made before it.
            Kind::Delta if self.reference >= self.frame => Err(FrameError::Identity),
            _ => Ok(()),
        }
    }

    pub fn encode(&self) -> Result<[u8; HEADER_BYTES], FrameError> {
        self.validate()?;
        let mut out = [0u8; HEADER_BYTES];
        out[0] = VERSION;
        out[1] = match self.kind {
            Kind::Key => KIND_KEY,
            Kind::Delta => KIND_DELTA,
        };
        out[2] = CODEC_H264;
        out[4..6].copy_from_slice(&self.width.to_be_bytes());
        out[6..8].copy_from_slice(&self.height.to_be_bytes());
        out[8..12].copy_from_slice(&self.stream.to_be_bytes());
        out[12..16].copy_from_slice(&self.frame.to_be_bytes());
        out[16..20].copy_from_slice(&self.reference.to_be_bytes());
        out[20..24].copy_from_slice(&self.size.to_be_bytes());
        out[24..32].copy_from_slice(&self.capture_micros.to_be_bytes());
        Ok(out)
    }

    /// The header at the start of `bytes`; `size` bytes of frame follow it.
    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError> {
        let header: &[u8; HEADER_BYTES] = bytes
            .get(..HEADER_BYTES)
            .and_then(|slice| slice.try_into().ok())
            .ok_or(FrameError::Short)?;
        if header[0] != VERSION || header[2] != CODEC_H264 || header[3] != 0 {
            return Err(FrameError::Unknown);
        }
        let kind = match header[1] {
            KIND_KEY => Kind::Key,
            KIND_DELTA => Kind::Delta,
            _ => return Err(FrameError::Unknown),
        };
        let u16_at = |at: usize| u16::from_be_bytes([header[at], header[at + 1]]);
        let u32_at = |at: usize| {
            u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
        };
        let mut capture = [0u8; 8];
        capture.copy_from_slice(&header[24..32]);
        let decoded = Self {
            kind,
            width: u16_at(4),
            height: u16_at(6),
            stream: u32_at(8),
            frame: u32_at(12),
            reference: u32_at(16),
            size: u32_at(20),
            capture_micros: u64::from_be_bytes(capture),
        };
        decoded.validate()?;
        Ok(decoded)
    }
}

/// Cuts a delta frame into datagrams of at most `max_datagram` bytes. Every
/// fragment but the last carries the same number of payload bytes, so a
/// receiver can check each fragment's place by its length.
pub fn fragment(
    stream: u32,
    frame: u32,
    reference: u32,
    capture_micros: u64,
    payload: &[u8],
    max_datagram: usize,
) -> Result<Vec<Vec<u8>>, FrameError> {
    let max_datagram = max_datagram.min(MAX_DATAGRAM_BYTES);
    if max_datagram < MIN_DATAGRAM_BYTES {
        return Err(FrameError::Size);
    }
    let size = u32::try_from(payload.len()).map_err(|_| FrameError::Size)?;
    if size == 0 || size > MAX_FRAME_BYTES {
        return Err(FrameError::Size);
    }
    let capacity = max_datagram - HEADER_BYTES;
    let count = u16::try_from(payload.len().div_ceil(capacity)).map_err(|_| FrameError::Size)?;
    let mut datagrams = Vec::with_capacity(usize::from(count));
    for (index, chunk) in payload.chunks(capacity).enumerate() {
        let header = FrameHeader {
            kind: Kind::Delta,
            fragment_index: index as u16,
            fragment_count: count,
            stream,
            frame,
            reference,
            size,
            capture_micros,
        }
        .encode()?;
        let mut datagram = Vec::with_capacity(HEADER_BYTES + chunk.len());
        datagram.extend_from_slice(&header);
        datagram.extend_from_slice(chunk);
        datagrams.push(datagram);
    }
    Ok(datagrams)
}

/// The payload length a fragment must have, given the length of the frame's
/// full fragments (`capacity`): full for all but the last, the rest for the
/// last. `None` if the numbering cannot belong to a frame cut that way.
pub fn expected_fragment_bytes(header: &FrameHeader, capacity: usize) -> Option<usize> {
    if header.kind != Kind::Delta || capacity == 0 {
        return None;
    }
    let size = header.size as usize;
    let count = usize::from(header.fragment_count);
    if size.div_ceil(capacity) != count {
        return None;
    }
    if usize::from(header.fragment_index) + 1 < count {
        Some(capacity)
    } else {
        Some(size - capacity * (count - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_round_trips_and_has_the_documented_layout() {
        let header = FrameHeader {
            kind: Kind::Delta,
            fragment_index: 2,
            fragment_count: 9,
            stream: 0x0102_0304,
            frame: 0x0a0b_0c0d,
            reference: 0x0a0b_0c00,
            size: 10_000,
            capture_micros: 0x1122_3344_5566_7788,
        };
        let bytes = header.encode().unwrap();
        assert_eq!(
            bytes,
            [
                1, 2, 1, 0, 0, 2, 0, 9, 1, 2, 3, 4, 0x0a, 0x0b, 0x0c, 0x0d, 0x0a, 0x0b, 0x0c, 0, 0,
                0, 0x27, 0x10, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88
            ]
        );
        assert_eq!(FrameHeader::decode(&bytes), Ok(header));
        let key = FrameHeader::key(5, 77, 123_456, 9);
        assert_eq!(FrameHeader::decode(&key.encode().unwrap()), Ok(key));
    }

    #[test]
    fn a_record_header_round_trips_and_has_the_documented_layout() {
        let header = RecordHeader {
            kind: Kind::Delta,
            width: 1920,
            height: 1080,
            stream: 0x0102_0304,
            frame: 0x0a0b_0c0d,
            reference: 0x0a0b_0c00,
            size: 10_000,
            capture_micros: 0x1122_3344_5566_7788,
        };
        let bytes = header.encode().unwrap();
        assert_eq!(
            bytes,
            [
                1, 2, 1, 0, 0x07, 0x80, 0x04, 0x38, 1, 2, 3, 4, 0x0a, 0x0b, 0x0c, 0x0d, 0x0a, 0x0b,
                0x0c, 0, 0, 0, 0x27, 0x10, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88
            ]
        );
        assert_eq!(RecordHeader::decode(&bytes), Ok(header));
        let key = RecordHeader {
            kind: Kind::Key,
            reference: header.frame,
            ..header
        };
        assert_eq!(RecordHeader::decode(&key.encode().unwrap()), Ok(key));

        assert_eq!(RecordHeader::decode(&bytes[..31]), Err(FrameError::Short));
        for (at, value) in [(0usize, 2u8), (1, 3), (1, 0), (2, 2), (3, 1)] {
            let mut damaged = bytes;
            damaged[at] = value;
            assert_eq!(RecordHeader::decode(&damaged), Err(FrameError::Unknown));
        }
        let refused = |change: fn(&mut RecordHeader)| {
            let mut changed = header;
            change(&mut changed);
            changed.encode().is_err()
        };
        assert!(refused(|header| header.stream = 0));
        assert!(refused(|header| header.frame = 0));
        assert!(refused(|header| header.reference = 0));
        assert!(refused(|header| header.reference = header.frame));
        assert!(refused(|header| header.kind = Kind::Key));
        assert!(refused(|header| header.size = 0));
        assert!(refused(|header| header.size = MAX_FRAME_BYTES + 1));
        assert!(refused(|header| header.width = 0));
        assert!(refused(|header| header.height = 0));
    }

    #[test]
    fn what_cannot_be_is_refused() {
        let good = FrameHeader {
            kind: Kind::Delta,
            fragment_index: 0,
            fragment_count: 3,
            stream: 1,
            frame: 10,
            reference: 4,
            size: 3000,
            capture_micros: 1,
        };
        let bytes = good.encode().unwrap();
        assert_eq!(FrameHeader::decode(&bytes[..31]), Err(FrameError::Short));
        for (at, value) in [(0usize, 2u8), (1, 3), (1, 0), (2, 2), (3, 1)] {
            let mut damaged = bytes;
            damaged[at] = value;
            assert_eq!(
                FrameHeader::decode(&damaged),
                Err(FrameError::Unknown),
                "byte {at}"
            );
        }
        let refused = |change: fn(&mut FrameHeader)| {
            let mut header = good;
            change(&mut header);
            header.encode().is_err()
        };
        assert!(refused(|header| header.stream = 0));
        assert!(refused(|header| header.frame = 0));
        assert!(refused(|header| header.reference = 0));
        assert!(
            refused(|header| header.reference = 10),
            "a delta cannot refer to itself"
        );
        assert!(refused(|header| header.reference = 11));
        assert!(refused(|header| header.size = 0));
        assert!(refused(|header| header.size = MAX_FRAME_BYTES + 1));
        assert!(refused(|header| header.fragment_count = 0));
        assert!(refused(|header| header.fragment_index = 3));
        assert!(refused(|header| {
            header.size = 2;
            header.fragment_count = 3;
        }));
        assert!(refused(|header| header.kind = Kind::Key));
        let mut key = FrameHeader::key(1, 10, 100, 0);
        key.fragment_count = 2;
        assert!(key.encode().is_err());
    }

    #[test]
    fn a_frame_is_cut_into_equal_fragments_and_a_rest() {
        let payload: Vec<u8> = (0..3000u32).map(|value| value as u8).collect();
        let datagrams = fragment(3, 20, 12, 99, &payload, 1200).unwrap();
        let capacity = 1200 - HEADER_BYTES;
        assert_eq!(datagrams.len(), 3);
        let mut joined = Vec::new();
        for (index, datagram) in datagrams.iter().enumerate() {
            assert!(datagram.len() <= 1200);
            let header = FrameHeader::decode(datagram).unwrap();
            assert_eq!(usize::from(header.fragment_index), index);
            assert_eq!(header.fragment_count, 3);
            assert_eq!((header.stream, header.frame, header.reference), (3, 20, 12));
            assert_eq!(header.size, 3000);
            assert_eq!(
                expected_fragment_bytes(&header, capacity),
                Some(datagram.len() - HEADER_BYTES)
            );
            joined.extend_from_slice(&datagram[HEADER_BYTES..]);
        }
        assert_eq!(joined, payload);
        // The path's limit is honoured below the ceiling, and the ceiling above it.
        assert!(
            fragment(3, 20, 12, 99, &payload, 600)
                .unwrap()
                .iter()
                .all(|d| d.len() <= 600)
        );
        assert!(
            fragment(3, 20, 12, 99, &payload, 9000)
                .unwrap()
                .iter()
                .all(|d| d.len() <= 1200)
        );
        assert_eq!(fragment(3, 20, 12, 99, &payload, 80), Err(FrameError::Size));
        assert_eq!(fragment(3, 20, 12, 99, &[], 1200), Err(FrameError::Size));
        // One byte over a full fragment makes a second one of one byte.
        let edge = vec![7u8; capacity + 1];
        let datagrams = fragment(3, 20, 12, 99, &edge, 1200).unwrap();
        assert_eq!(datagrams.len(), 2);
        assert_eq!(datagrams[1].len(), HEADER_BYTES + 1);
    }

    #[test]
    fn a_fragment_whose_length_does_not_fit_its_frame_is_recognised() {
        let payload = vec![1u8; 2500];
        let datagrams = fragment(3, 20, 12, 99, &payload, 1200).unwrap();
        let capacity = 1200 - HEADER_BYTES;
        let last = FrameHeader::decode(&datagrams[2]).unwrap();
        assert_eq!(
            expected_fragment_bytes(&last, capacity),
            Some(2500 - 2 * capacity)
        );
        // A count that the size and the fragment length cannot produce.
        let mut wrong = last;
        wrong.fragment_count = 4;
        assert_eq!(expected_fragment_bytes(&wrong, capacity), None);
        assert_eq!(
            expected_fragment_bytes(&FrameHeader::key(1, 2, 3, 4), capacity),
            None
        );
    }
}
