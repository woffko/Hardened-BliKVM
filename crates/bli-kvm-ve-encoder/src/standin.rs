//! A stand-in for the engine in software: it writes slice data that any
//! decoder accepts, so that everything around the engine can be tested where
//! there is none. A key frame is made of I_PCM macroblocks, which carry the
//! picture's samples as they are; any other frame skips every macroblock,
//! which repeats the frame before it.

use std::io;

use crate::{
    abi,
    bits::{BitSink, Put},
    device::{Device, Made, Pass, Picture, Reference, Sizes},
    headers::RawUnit,
    nv12,
    rate::Search,
    sets::Sets,
};

/// `mb_type` of I_PCM in an I slice.
const I_PCM: u32 = 25;

/// Writes the slice's data after its header, up to the end of the unit, for
/// a picture of `columns` by `rows` macroblocks. `luma` and `chroma` are
/// NV12 planes of `stride` bytes a row that hold at least that size.
pub fn slice_data(
    sink: &mut BitSink,
    columns: usize,
    rows: usize,
    key: bool,
    luma: &[u8],
    chroma: &[u8],
    stride: usize,
) {
    if key {
        let mut samples = [0u8; 384];
        for row in 0..rows {
            for column in 0..columns {
                sink.put(Put::ue(I_PCM));
                sink.align();
                for line in 0..16 {
                    let start = (row * 16 + line) * stride + column * 16;
                    samples[line * 16..line * 16 + 16].copy_from_slice(&luma[start..start + 16]);
                }
                // Cb, then Cr: the interleaved plane taken apart.
                for line in 0..8 {
                    let start = (row * 8 + line) * stride + column * 16;
                    for x in 0..8 {
                        samples[256 + line * 8 + x] = chroma[start + 2 * x];
                        samples[320 + line * 8 + x] = chroma[start + 2 * x + 1];
                    }
                }
                sink.bytes(&samples);
            }
        }
    } else {
        // `mb_skip_run`: every macroblock of the picture.
        sink.put(Put::ue((columns * rows) as u32));
    }
    // The stop bit, and zeros to the byte's end.
    sink.put(Put::new(1, 1));
    sink.align();
}

/// The driver, stood in for: the same requests are refused, the same sets
/// of buffers are read and written, and what comes out is a stream any
/// decoder takes. A key frame carries the picture as it is; any other
/// frame says that nothing changed, whatever the picture.
#[derive(Default)]
pub struct StandinDevice {
    geometry: Option<(u32, u32)>,
    sets: Sets,
    picture: Vec<u8>,
    have_picture: bool,
    /// Every pass that was made: its flags as the driver's interface has
    /// them, its QP, and the set it read.
    pub passes: Vec<(u32, u32, usize)>,
    /// How many bytes a frame should have, for a test of the rate: zero
    /// bytes are appended to the unit up to that size, which a decoder
    /// skips. Given the pass and the picture.
    #[allow(clippy::type_complexity)]
    pub size: Option<Box<dyn FnMut(&Pass<'_>, &[u8]) -> usize + Send>>,
    /// The passes left until one fails as the engine's would.
    pub fail_in: Option<usize>,
    /// The passes left until every one fails.
    pub fail_from: Option<usize>,
}

impl StandinDevice {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Device for StandinDevice {
    fn configure(&mut self, width: u32, height: u32, cabac: bool) -> io::Result<Sizes> {
        self.geometry = None;
        self.sets.reset();
        self.have_picture = false;
        let refused = io::Error::from(io::ErrorKind::InvalidInput);
        if !(abi::MIN_WIDTH..=abi::MAX_WIDTH).contains(&width)
            || !(abi::MIN_HEIGHT..=abi::MAX_HEIGHT).contains(&height)
            || width % 2 != 0
            || height % 2 != 0
            || cabac
        {
            return Err(refused);
        }
        self.geometry = Some((width, height));
        let picture_bytes = nv12::picture_bytes(width as usize, height as usize);
        Ok(Sizes {
            picture_bytes,
            stream_bytes: picture_bytes.div_ceil(1 << 20) << 20,
        })
    }

    fn encode(
        &mut self,
        pass: &Pass<'_>,
        units: &[RawUnit],
        stream: &mut Vec<u8>,
    ) -> io::Result<Made> {
        let refused = || io::Error::from(io::ErrorKind::InvalidInput);
        let (width, height) = self.geometry.ok_or_else(refused)?;
        let (columns, rows) = (
            (width as usize).div_ceil(16),
            (height as usize).div_ceil(16),
        );
        let picture_bytes = columns * 16 * rows * 16 * 3 / 2;
        if pass.qp > abi::MAX_QP
            || units.is_empty()
            || units.len() > abi::MAX_UNITS
            || units.iter().any(|unit| unit.bits > 8 * abi::MAX_UNIT_BYTES)
        {
            return Err(refused());
        }
        let flags = if pass.key { abi::KEY } else { 0 }
            | if pass.search == Search::Wide {
                abi::WIDE
            } else {
                0
            }
            | match pass.picture {
                Picture::Again => abi::AGAIN,
                Picture::New { reference, .. } => match reference {
                    Reference::Last => 0,
                    Reference::Earlier => abi::EARLIER,
                    Reference::Stay => abi::STAY,
                },
            };
        let sets = self
            .sets
            .check(flags, self.have_picture)
            .ok_or_else(refused)?;
        if let Picture::New { nv12, .. } = pass.picture {
            if nv12.len() != picture_bytes {
                return Err(refused());
            }
            self.picture.clear();
            self.picture.extend_from_slice(nv12);
            self.have_picture = true;
        }
        if let Some(left) = self.fail_in.as_mut() {
            if *left == 0 {
                // The engine did not finish: nothing is a frame any more.
                self.fail_in = None;
                self.sets.reset();
                return Err(io::Error::from(io::ErrorKind::TimedOut));
            }
            *left -= 1;
        }
        match self.fail_from.as_mut() {
            Some(0) => {
                self.sets.reset();
                return Err(io::Error::from(io::ErrorKind::TimedOut));
            }
            Some(left) => *left -= 1,
            None => {}
        }

        let mut sink = BitSink::new();
        for unit in units {
            sink.set_prevention(false);
            sink.put(Put::new(0, 16));
            sink.put(Put::new(1, 16));
            sink.put(Put::new(unit.header as u32, 8));
            sink.set_prevention(true);
            let mut done = 0;
            while done < unit.bits {
                let take = (unit.bits - done).min(8);
                let byte = unit.payload[done / 8] as u32;
                sink.put(Put::new(byte >> (8 - take), take as u8));
                done += take;
            }
        }
        let stride = columns * 16;
        let (luma, chroma) = self.picture.split_at(stride * rows * 16);
        slice_data(&mut sink, columns, rows, pass.key, luma, chroma, stride);
        stream.clear();
        stream.extend_from_slice(&sink.into_bytes());
        if let Some(size) = self.size.as_mut() {
            let wanted = size(pass, &self.picture);
            if wanted > stream.len() {
                stream.resize(wanted, 0);
            }
        }
        self.sets.done(flags, &sets);
        self.passes.push((flags, pass.qp, sets.reference));
        Ok(Made {
            status: 1,
            ..Made::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::{BitSource, without_prevention};

    #[test]
    fn a_key_frame_carries_the_picture_as_it_is() {
        // A picture of 64 by 48 in which no two samples nearby are alike.
        struct Planes {
            luma: Vec<u8>,
            chroma: Vec<u8>,
        }
        let mut picture = Planes {
            luma: (0..64 * 48u32)
                .map(|at| (at * 7 + at / 64 * 13) as u8)
                .collect(),
            chroma: (0..64 * 24u32).map(|at| (at * 11 + 5) as u8).collect(),
        };
        // Zero samples in a row call for emulation prevention.
        picture.luma[16..96].fill(0);
        let mut sink = BitSink::new();
        sink.set_prevention(true);
        slice_data(&mut sink, 4, 3, true, &picture.luma, &picture.chroma, 64);
        let written = sink.into_bytes();
        let payload = without_prevention(&written);
        assert!(written.len() > payload.len());
        let mut source = BitSource::new(&payload);
        for row in 0..3 {
            for column in 0..4 {
                assert_eq!(source.ue(), Some(I_PCM));
                source.align();
                let at = source.position() / 8;
                let samples = &payload[at..at + 384];
                for line in 0..16 {
                    let start = (row * 16 + line) * 64 + column * 16;
                    assert_eq!(
                        &samples[line * 16..line * 16 + 16],
                        &picture.luma[start..start + 16]
                    );
                }
                assert_eq!(samples[256], picture.chroma[row * 8 * 64 + column * 16]);
                assert_eq!(samples[320], picture.chroma[row * 8 * 64 + column * 16 + 1]);
                for _ in 0..384 {
                    source.bits(8);
                }
            }
        }
        assert_eq!(source.bit(), Some(1));
        assert_eq!(source.position().div_ceil(8), payload.len());
    }

    #[test]
    fn any_other_frame_skips_every_macroblock() {
        let mut sink = BitSink::new();
        slice_data(&mut sink, 120, 68, false, &[], &[], 1920);
        let written = sink.into_bytes();
        let mut source = BitSource::new(&written);
        assert_eq!(source.ue(), Some(120 * 68));
        assert_eq!(source.bit(), Some(1));
        assert_eq!(written.len(), 4);
    }
}
