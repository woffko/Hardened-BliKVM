//! Bits of an H.264 stream: what is handed to the engine's bit writer, and a
//! writer in software that does the same for the stand-in.

/// One write to a bit writer: the low `bits` bits of `value`, the most
/// significant of them first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Put {
    pub value: u32,
    pub bits: u8,
}

/// The engine's bit writer is given at most this many bits at a time: both
/// open sources stay within it for everything but a start code.
pub const MAX_PUT_BITS: u8 = 16;

impl Put {
    pub fn new(value: u32, bits: u8) -> Self {
        assert!((1..=32).contains(&bits), "a put has 1 to 32 bits");
        assert!(
            bits == 32 || value >> bits == 0,
            "the value has more bits than the put"
        );
        Self { value, bits }
    }

    /// An unsigned Exp-Golomb code.
    pub fn ue(value: u32) -> Self {
        assert!(
            value < u32::MAX >> 16,
            "an Exp-Golomb value of at most 16 bits"
        );
        let coded = value + 1;
        let length = 32 - coded.leading_zeros();
        // `length - 1` zero bits, then the `length` bits of the value plus one.
        Self::new(coded, (2 * length - 1) as u8)
    }

    /// A signed Exp-Golomb code: 1, -1, 2, -2, ... follow 0.
    pub fn se(value: i32) -> Self {
        let coded = if value > 0 {
            2 * value as u32 - 1
        } else {
            2 * value.unsigned_abs()
        };
        Self::ue(coded)
    }

    /// The same bits in pieces of at most `MAX_PUT_BITS`.
    pub fn pieces(self) -> impl Iterator<Item = Put> {
        let mut left = self.bits;
        std::iter::from_fn(move || {
            if left == 0 {
                return None;
            }
            let take = left.min(MAX_PUT_BITS);
            left -= take;
            let mask = if take == 32 {
                u32::MAX
            } else {
                (1 << take) - 1
            };
            Some(Put {
                value: (self.value >> left) & mask,
                bits: take,
            })
        })
    }
}

/// Collects bits into the bytes of a stream. With prevention on it inserts
/// the emulation prevention byte, as the engine does for what it writes.
#[derive(Debug, Default)]
pub struct BitSink {
    bytes: Vec<u8>,
    pending: u8,
    pending_bits: u8,
    zeros: u8,
    prevent: bool,
}

impl BitSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `00 00 0x` is kept from appearing in what is written next.
    pub fn set_prevention(&mut self, prevent: bool) {
        self.prevent = prevent;
        self.zeros = 0;
    }

    pub fn put(&mut self, put: Put) {
        for index in (0..put.bits).rev() {
            let bit = ((put.value >> index) & 1) as u8;
            self.pending = (self.pending << 1) | bit;
            self.pending_bits += 1;
            if self.pending_bits == 8 {
                let byte = self.pending;
                self.pending = 0;
                self.pending_bits = 0;
                self.byte(byte);
            }
        }
    }

    /// Whole bytes at a byte boundary.
    pub fn bytes(&mut self, bytes: &[u8]) {
        assert!(self.aligned(), "bytes are written at a byte boundary");
        for &byte in bytes {
            self.byte(byte);
        }
    }

    fn byte(&mut self, byte: u8) {
        if self.prevent && self.zeros >= 2 && byte <= 3 {
            self.bytes.push(3);
            self.zeros = 0;
        }
        self.bytes.push(byte);
        self.zeros = if byte == 0 {
            self.zeros.saturating_add(1)
        } else {
            0
        };
    }

    pub fn aligned(&self) -> bool {
        self.pending_bits == 0
    }

    /// Zero bits up to the next byte boundary.
    pub fn align(&mut self) {
        while !self.aligned() {
            self.put(Put::new(0, 1));
        }
    }

    /// The bits written so far, emulation prevention bytes included.
    pub fn bit_length(&self) -> usize {
        self.bytes.len() * 8 + self.pending_bits as usize
    }

    /// The whole bytes written so far.
    pub fn written(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        assert!(self.aligned(), "a stream ends at a byte boundary");
        self.bytes
    }
}

/// Reads bits back; for the tests that parse what the writers made.
#[derive(Debug)]
pub struct BitSource<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> BitSource<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    pub fn bit(&mut self) -> Option<u32> {
        let byte = *self.bytes.get(self.position / 8)?;
        let bit = (byte >> (7 - self.position % 8)) & 1;
        self.position += 1;
        Some(bit as u32)
    }

    pub fn bits(&mut self, count: u8) -> Option<u32> {
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1) | self.bit()?;
        }
        Some(value)
    }

    pub fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1 << zeros) - 1 + self.bits(zeros)?)
    }

    pub fn se(&mut self) -> Option<i32> {
        let coded = self.ue()?;
        Some(if coded % 2 == 1 {
            coded.div_ceil(2) as i32
        } else {
            -((coded / 2) as i32)
        })
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn aligned(&self) -> bool {
        self.position % 8 == 0
    }

    pub fn align(&mut self) {
        self.position = self.position.div_ceil(8) * 8;
    }
}

/// The payload of a NAL unit without its emulation prevention bytes.
pub fn without_prevention(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len());
    let mut zeros = 0;
    for &byte in payload {
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb_codes_are_the_standards() {
        // Table 9-2 of the standard: 0 is "1", 1 is "010", 2 is "011", ...
        assert_eq!(Put::ue(0), Put { value: 1, bits: 1 });
        assert_eq!(Put::ue(1), Put { value: 2, bits: 3 });
        assert_eq!(Put::ue(2), Put { value: 3, bits: 3 });
        assert_eq!(Put::ue(3), Put { value: 4, bits: 5 });
        assert_eq!(Put::ue(25), Put { value: 26, bits: 9 });
        assert_eq!(
            Put::ue(8159),
            Put {
                value: 8160,
                bits: 25
            }
        );
        // Table 9-3: 1, -1, 2, -2.
        assert_eq!(Put::se(0), Put::ue(0));
        assert_eq!(Put::se(1), Put::ue(1));
        assert_eq!(Put::se(-1), Put::ue(2));
        assert_eq!(Put::se(2), Put::ue(3));
        assert_eq!(Put::se(-26), Put::ue(52));
    }

    #[test]
    fn what_is_written_is_read_back() {
        let mut sink = BitSink::new();
        let values = [0u32, 1, 2, 7, 25, 119, 8159, 65534];
        for value in values {
            sink.put(Put::ue(value));
            sink.put(Put::se(value as i32 / 2 - 100));
        }
        sink.put(Put::new(1, 1));
        sink.align();
        let bytes = sink.into_bytes();
        let mut source = BitSource::new(&bytes);
        for value in values {
            assert_eq!(source.ue(), Some(value));
            assert_eq!(source.se(), Some(value as i32 / 2 - 100));
        }
        assert_eq!(source.bit(), Some(1));
    }

    #[test]
    fn pieces_carry_the_same_bits() {
        for put in [Put::ue(8159), Put::new(0x1234_5678, 32), Put::new(5, 3)] {
            let mut whole = BitSink::new();
            whole.put(put);
            whole.align();
            let mut split = BitSink::new();
            for piece in put.pieces() {
                assert!(piece.bits <= MAX_PUT_BITS);
                split.put(piece);
            }
            split.align();
            assert_eq!(whole.into_bytes(), split.into_bytes());
        }
    }

    #[test]
    fn three_zero_bytes_never_follow_each_other_under_prevention() {
        let mut sink = BitSink::new();
        sink.set_prevention(true);
        let payload = [0, 0, 0, 0, 1, 0, 0, 2, 0, 0, 3, 0, 0, 4, 0, 0];
        sink.bytes(&payload);
        let bytes = sink.into_bytes();
        assert_eq!(
            bytes,
            [0, 0, 3, 0, 0, 3, 1, 0, 0, 3, 2, 0, 0, 3, 3, 0, 0, 4, 0, 0]
        );
        assert!(
            !bytes
                .windows(3)
                .any(|window| window[0] == 0 && window[1] == 0 && window[2] <= 2)
        );
        assert_eq!(without_prevention(&bytes), payload);
    }

    #[test]
    fn without_prevention_a_start_code_is_written_as_it_is() {
        let mut sink = BitSink::new();
        sink.put(Put::new(0, 16));
        sink.put(Put::new(1, 16));
        assert_eq!(sink.bit_length(), 32);
        assert_eq!(sink.into_bytes(), [0, 0, 0, 1]);
    }
}
