//! A message in frames of one HID report each.
//!
//! The agent's reports carry 63 bytes after their report ID, each way. A
//! message is cut into frames of that size: each says the protocol's
//! version, whether it is the message's first and last, the message's number
//! and the frame's place in it, how many of its bytes are the message's, and
//! a CRC-32 of the rest. Reports on one interface arrive in order or not at
//! all, so a frame that is not the next one, or a frame that is no frame,
//! drops the message it would belong to; the next first frame begins a new
//! one.

use thiserror::Error;

/// The bytes of one frame: a report's, after its report ID.
pub const FRAME_BYTES: usize = 63;
/// The frame's own bytes before the message's.
const HEADER_BYTES: usize = 8;
/// The CRC-32 at the end.
const CRC_BYTES: usize = 4;
/// A message's bytes in one frame.
pub const PAYLOAD_BYTES: usize = FRAME_BYTES - HEADER_BYTES - CRC_BYTES;
/// The largest message.
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024;
/// The frames' version: a frame of another is refused.
pub const FRAME_VERSION: u8 = 1;

const FIRST: u8 = 1 << 0;
const LAST: u8 = 1 << 1;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("a message of {0} bytes is longer than {MAX_MESSAGE_BYTES}")]
    TooLong(usize),
    #[error("an empty message")]
    Empty,
}

/// Why a report was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dropped {
    /// Not a report of the agent's length or ID, a CRC that does not match,
    /// or a length out of range.
    NotAFrame,
    /// A frame of another version.
    Version(u8),
    /// A frame that is not the next one of the message being put together,
    /// or not a first one when none is: the message is dropped.
    OutOfOrder,
    /// More bytes than a message may have: dropped.
    TooLong,
}

/// The CRC-32 (IEEE 802.3, as zlib's) of `bytes`.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// The frames that carry `message` as message number `number`.
pub fn frames(number: u16, message: &[u8]) -> Result<Vec<[u8; FRAME_BYTES]>, FrameError> {
    if message.is_empty() {
        return Err(FrameError::Empty);
    }
    if message.len() > MAX_MESSAGE_BYTES {
        return Err(FrameError::TooLong(message.len()));
    }
    let chunks: Vec<&[u8]> = message.chunks(PAYLOAD_BYTES).collect();
    let count = chunks.len();
    Ok(chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let mut frame = [0_u8; FRAME_BYTES];
            let mut flags = 0;
            if index == 0 {
                flags |= FIRST;
            }
            if index + 1 == count {
                flags |= LAST;
            }
            frame[0] = FRAME_VERSION;
            frame[1] = flags;
            frame[2..4].copy_from_slice(&number.to_le_bytes());
            frame[4..6].copy_from_slice(&(index as u16).to_le_bytes());
            frame[6] = chunk.len() as u8;
            frame[HEADER_BYTES..HEADER_BYTES + chunk.len()].copy_from_slice(chunk);
            let crc = crc32(&frame[..FRAME_BYTES - CRC_BYTES]);
            frame[FRAME_BYTES - CRC_BYTES..].copy_from_slice(&crc.to_le_bytes());
            frame
        })
        .collect())
}

/// Puts messages together from frames as they come.
#[derive(Debug, Default)]
pub struct Assembler {
    /// The message being put together: its number, the next frame's place
    /// and its bytes so far.
    current: Option<(u16, u16, Vec<u8>)>,
}

impl Assembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes one frame: a whole message once its last frame came, nothing
    /// while it is still coming, or why the frame was not taken.
    pub fn push(&mut self, frame: &[u8]) -> Result<Option<(u16, Vec<u8>)>, Dropped> {
        if frame.len() != FRAME_BYTES {
            return Err(Dropped::NotAFrame);
        }
        let crc = u32::from_le_bytes(frame[FRAME_BYTES - CRC_BYTES..].try_into().unwrap());
        if crc != crc32(&frame[..FRAME_BYTES - CRC_BYTES]) {
            return Err(Dropped::NotAFrame);
        }
        if frame[0] != FRAME_VERSION {
            return Err(Dropped::Version(frame[0]));
        }
        let flags = frame[1];
        let number = u16::from_le_bytes([frame[2], frame[3]]);
        let index = u16::from_le_bytes([frame[4], frame[5]]);
        let length = usize::from(frame[6]);
        if length == 0 || length > PAYLOAD_BYTES || frame[7] != 0 {
            return Err(Dropped::NotAFrame);
        }
        let payload = &frame[HEADER_BYTES..HEADER_BYTES + length];
        if flags & FIRST != 0 {
            if index != 0 {
                self.current = None;
                return Err(Dropped::OutOfOrder);
            }
            // A first frame begins a message, whatever was coming.
            self.current = Some((number, 0, Vec::new()));
        }
        let Some((current, next, bytes)) = self.current.as_mut() else {
            return Err(Dropped::OutOfOrder);
        };
        if *current != number || *next != index {
            self.current = None;
            return Err(Dropped::OutOfOrder);
        }
        if bytes.len() + payload.len() > MAX_MESSAGE_BYTES {
            self.current = None;
            return Err(Dropped::TooLong);
        }
        bytes.extend_from_slice(payload);
        *next = next.wrapping_add(1);
        if flags & LAST != 0 {
            let (number, _, bytes) = self.current.take().unwrap();
            return Ok(Some((number, bytes)));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deliver(assembler: &mut Assembler, frames: &[[u8; FRAME_BYTES]]) -> Vec<(u16, Vec<u8>)> {
        frames
            .iter()
            .filter_map(|frame| assembler.push(frame).ok().flatten())
            .collect()
    }

    #[test]
    fn the_crc_is_zlibs() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn messages_of_every_length_come_back_whole() {
        let mut assembler = Assembler::new();
        for length in [
            1,
            PAYLOAD_BYTES - 1,
            PAYLOAD_BYTES,
            PAYLOAD_BYTES + 1,
            1000,
            MAX_MESSAGE_BYTES,
        ] {
            let message: Vec<u8> = (0..length).map(|at| (at * 7 % 251) as u8).collect();
            let sent = frames(length as u16, &message).unwrap();
            assert_eq!(sent.len(), length.div_ceil(PAYLOAD_BYTES));
            assert_eq!(
                deliver(&mut assembler, &sent),
                vec![(length as u16, message)],
                "{length}"
            );
        }
    }

    #[test]
    fn an_empty_or_too_long_message_is_not_sent() {
        assert_eq!(frames(1, b""), Err(FrameError::Empty));
        assert_eq!(
            frames(1, &vec![0; MAX_MESSAGE_BYTES + 1]),
            Err(FrameError::TooLong(MAX_MESSAGE_BYTES + 1))
        );
    }

    #[test]
    fn a_frame_that_is_no_frame_is_refused() {
        let mut assembler = Assembler::new();
        let mut frame = frames(1, b"hello").unwrap()[0];
        assert_eq!(
            assembler.push(&frame[..FRAME_BYTES - 1]),
            Err(Dropped::NotAFrame)
        );
        frame[10] ^= 1;
        assert_eq!(assembler.push(&frame), Err(Dropped::NotAFrame));
        let mut other = frames(1, b"hello").unwrap()[0];
        other[0] = 2;
        let crc = crc32(&other[..FRAME_BYTES - CRC_BYTES]);
        other[FRAME_BYTES - CRC_BYTES..].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(assembler.push(&other), Err(Dropped::Version(2)));
    }

    #[test]
    fn a_lost_frame_drops_its_message_and_the_next_one_comes() {
        let mut assembler = Assembler::new();
        let first = frames(7, &[1; 200]).unwrap();
        let second = frames(8, &[2; 120]).unwrap();
        // The second frame of the first message is lost.
        let mut sent = vec![first[0]];
        sent.extend_from_slice(&first[2..]);
        sent.extend_from_slice(&second);
        assert_eq!(deliver(&mut assembler, &sent), vec![(8, vec![2; 120])]);
    }

    #[test]
    fn frames_out_of_order_drop_their_message() {
        let mut assembler = Assembler::new();
        let message = frames(3, &[3; 160]).unwrap();
        let sent = [message[0], message[2], message[1], message[3]];
        assert_eq!(deliver(&mut assembler, &sent), vec![]);
        assert_eq!(
            deliver(&mut assembler, &frames(4, b"after").unwrap()),
            vec![(4, b"after".to_vec())]
        );
    }

    #[test]
    fn two_messages_interleaved_are_both_dropped() {
        let mut assembler = Assembler::new();
        let first = frames(1, &[1; 120]).unwrap();
        let second = frames(2, &[2; 120]).unwrap();
        let sent: Vec<_> = first
            .iter()
            .zip(&second)
            .flat_map(|(one, two)| [*one, *two])
            .collect();
        assert_eq!(deliver(&mut assembler, &sent), vec![]);
        assert_eq!(
            deliver(&mut assembler, &frames(3, b"after").unwrap()),
            vec![(3, b"after".to_vec())]
        );
    }

    #[test]
    fn a_frame_without_a_first_one_is_out_of_order() {
        let mut assembler = Assembler::new();
        let message = frames(5, &[5; 100]).unwrap();
        assert_eq!(assembler.push(&message[1]), Err(Dropped::OutOfOrder));
    }

    #[test]
    fn a_first_frame_begins_a_new_message_whatever_was_coming() {
        let mut assembler = Assembler::new();
        let first = frames(1, &[1; 200]).unwrap();
        let second = frames(2, &[2; 60]).unwrap();
        let mut sent = first[..2].to_vec();
        sent.extend_from_slice(&second);
        assert_eq!(deliver(&mut assembler, &sent), vec![(2, vec![2; 60])]);
        // A later frame of the first one, after, is out of order.
        assert_eq!(assembler.push(&first[2]), Err(Dropped::OutOfOrder));
    }
}
