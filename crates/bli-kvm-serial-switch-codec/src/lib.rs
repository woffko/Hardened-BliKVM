#![no_std]
#![forbid(unsafe_code)]

//! Pure codec for the bounded four-port BliKVM serial-switch frame grammar.
//!
//! The pinned vendor source constructs ten initialized select-command bytes:
//! `SW1\r\nG0{port}gA`. Its status loop uses byte zero as `G`, byte two as the
//! port, and byte four as `A`; this crate deliberately narrows that permissive
//! check to the exact five-byte token `G0{port}gA`. It has no device discovery,
//! TTY, termios, filesystem, clock, timeout, or I/O capability. In particular,
//! it never reproduces the vendor's two unwritten padding bytes.

use core::fmt;

pub const SELECT_FRAME_LENGTH: usize = 10;
pub const STATUS_FRAME_LENGTH: usize = 5;

const SELECT_TEMPLATE: [u8; SELECT_FRAME_LENGTH] = *b"SW1\r\nG01gA";
const STATUS_TEMPLATE: [u8; STATUS_FRAME_LENGTH] = *b"G01gA";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SwitchPort {
    One = 1,
    Two = 2,
    Three = 3,
    Four = 4,
}

impl SwitchPort {
    pub const fn number(self) -> u8 {
        self as u8
    }

    const fn ascii_digit(self) -> u8 {
        b'0' + self.number()
    }

    const fn from_ascii_digit(value: u8) -> Option<Self> {
        match value {
            b'1' => Some(Self::One),
            b'2' => Some(Self::Two),
            b'3' => Some(Self::Three),
            b'4' => Some(Self::Four),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPortNumber {
    value: u8,
}

impl InvalidPortNumber {
    pub const fn value(self) -> u8 {
        self.value
    }
}

impl fmt::Display for InvalidPortNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "serial-switch port {} is outside 1 through 4",
            self.value
        )
    }
}

impl TryFrom<u8> for SwitchPort {
    type Error = InvalidPortNumber;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::One),
            2 => Ok(Self::Two),
            3 => Ok(Self::Three),
            4 => Ok(Self::Four),
            _ => Err(InvalidPortNumber { value }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    InvalidLength { expected: usize, actual: usize },
    InvalidPreamble,
    InvalidPortByte(u8),
    InvalidTerminator,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength { expected, actual } => write!(
                formatter,
                "serial-switch frame length must be {expected} bytes, got {actual}"
            ),
            Self::InvalidPreamble => formatter.write_str("serial-switch frame preamble is invalid"),
            Self::InvalidPortByte(_) => {
                formatter.write_str("serial-switch frame port must be ASCII 1 through 4")
            }
            Self::InvalidTerminator => {
                formatter.write_str("serial-switch frame terminator is invalid")
            }
        }
    }
}

/// Encode one exact ten-byte switch-selection command.
pub const fn encode_select(port: SwitchPort) -> [u8; SELECT_FRAME_LENGTH] {
    let mut frame = SELECT_TEMPLATE;
    frame[7] = port.ascii_digit();
    frame
}

/// Validate and decode one exact switch-selection command.
pub fn decode_select(frame: &[u8]) -> Result<SwitchPort, DecodeError> {
    if frame.len() != SELECT_FRAME_LENGTH {
        return Err(DecodeError::InvalidLength {
            expected: SELECT_FRAME_LENGTH,
            actual: frame.len(),
        });
    }
    if frame[..7] != SELECT_TEMPLATE[..7] {
        return Err(DecodeError::InvalidPreamble);
    }
    let port =
        SwitchPort::from_ascii_digit(frame[7]).ok_or(DecodeError::InvalidPortByte(frame[7]))?;
    if frame[8..] != SELECT_TEMPLATE[8..] {
        return Err(DecodeError::InvalidTerminator);
    }
    Ok(port)
}

/// Validate and decode one exact five-byte switch status frame.
pub fn decode_status(frame: &[u8]) -> Result<SwitchPort, DecodeError> {
    if frame.len() != STATUS_FRAME_LENGTH {
        return Err(DecodeError::InvalidLength {
            expected: STATUS_FRAME_LENGTH,
            actual: frame.len(),
        });
    }
    if frame[..2] != STATUS_TEMPLATE[..2] {
        return Err(DecodeError::InvalidPreamble);
    }
    let port =
        SwitchPort::from_ascii_digit(frame[2]).ok_or(DecodeError::InvalidPortByte(frame[2]))?;
    if frame[3..] != STATUS_TEMPLATE[3..] {
        return Err(DecodeError::InvalidTerminator);
    }
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASES: [(
        SwitchPort,
        [u8; SELECT_FRAME_LENGTH],
        [u8; STATUS_FRAME_LENGTH],
    ); 4] = [
        (SwitchPort::One, *b"SW1\r\nG01gA", *b"G01gA"),
        (SwitchPort::Two, *b"SW1\r\nG02gA", *b"G02gA"),
        (SwitchPort::Three, *b"SW1\r\nG03gA", *b"G03gA"),
        (SwitchPort::Four, *b"SW1\r\nG04gA", *b"G04gA"),
    ];

    #[test]
    fn all_four_ports_have_exact_deterministic_frames() {
        for (port, select, status) in CASES {
            assert_eq!(encode_select(port), select);
            assert_eq!(decode_select(&select), Ok(port));
            assert_eq!(decode_status(&status), Ok(port));
            assert_eq!(port.number(), select[7] - b'0');
        }
    }

    #[test]
    fn select_encoding_is_const_and_contains_no_padding() {
        const FRAME: [u8; SELECT_FRAME_LENGTH] = encode_select(SwitchPort::Four);
        assert_eq!(FRAME, *b"SW1\r\nG04gA");

        let mut vendor_sized_buffer = [0_u8; 12];
        vendor_sized_buffer[..SELECT_FRAME_LENGTH].copy_from_slice(&FRAME);
        assert_eq!(
            decode_select(&vendor_sized_buffer),
            Err(DecodeError::InvalidLength {
                expected: SELECT_FRAME_LENGTH,
                actual: 12,
            })
        );
    }

    #[test]
    fn numeric_port_conversion_is_bounded() {
        for (number, port) in [
            (1, SwitchPort::One),
            (2, SwitchPort::Two),
            (3, SwitchPort::Three),
            (4, SwitchPort::Four),
        ] {
            assert_eq!(SwitchPort::try_from(number), Ok(port));
        }
        for invalid in [0, 5, u8::MAX] {
            let error = SwitchPort::try_from(invalid).unwrap_err();
            assert_eq!(error.value(), invalid);
        }
    }

    #[test]
    fn decoders_reject_every_non_port_byte() {
        for byte in u8::MIN..=u8::MAX {
            let mut status = STATUS_TEMPLATE;
            status[2] = byte;
            let decoded_status = decode_status(&status);
            let mut select = SELECT_TEMPLATE;
            select[7] = byte;
            let decoded_select = decode_select(&select);
            match byte {
                b'1' => {
                    assert_eq!(decoded_status, Ok(SwitchPort::One));
                    assert_eq!(decoded_select, Ok(SwitchPort::One));
                }
                b'2' => {
                    assert_eq!(decoded_status, Ok(SwitchPort::Two));
                    assert_eq!(decoded_select, Ok(SwitchPort::Two));
                }
                b'3' => {
                    assert_eq!(decoded_status, Ok(SwitchPort::Three));
                    assert_eq!(decoded_select, Ok(SwitchPort::Three));
                }
                b'4' => {
                    assert_eq!(decoded_status, Ok(SwitchPort::Four));
                    assert_eq!(decoded_select, Ok(SwitchPort::Four));
                }
                _ => {
                    assert_eq!(decoded_status, Err(DecodeError::InvalidPortByte(byte)));
                    assert_eq!(decoded_select, Err(DecodeError::InvalidPortByte(byte)));
                }
            }
        }
    }

    #[test]
    fn fixed_bytes_are_not_tolerated_when_mutated() {
        for index in [0, 1, 2, 3, 4, 5, 6, 8, 9] {
            let mut frame = encode_select(SwitchPort::One);
            frame[index] ^= 1;
            assert!(
                decode_select(&frame).is_err(),
                "select byte {index} was accepted"
            );
        }
        for index in [0, 1, 3, 4] {
            let mut frame = STATUS_TEMPLATE;
            frame[index] ^= 1;
            assert!(
                decode_status(&frame).is_err(),
                "status byte {index} was accepted"
            );
        }
    }

    #[test]
    fn short_trailing_and_noisy_frames_fail_closed() {
        for frame in [
            &b""[..],
            &b"G"[..],
            &b"G0"[..],
            &b"G01"[..],
            &b"G01g"[..],
            &b"G01gA\n"[..],
            &b"SW1\r\nG01gA"[..],
        ] {
            assert!(matches!(
                decode_status(frame),
                Err(DecodeError::InvalidLength { .. })
            ));
        }

        for frame in [
            &b""[..],
            &b"SW1\r\nG01g"[..],
            &b"SW1\r\nG01gA\0"[..],
            &b"xSW1\r\nG01gA"[..],
        ] {
            assert!(matches!(
                decode_select(frame),
                Err(DecodeError::InvalidLength { .. })
            ));
        }
    }
}
