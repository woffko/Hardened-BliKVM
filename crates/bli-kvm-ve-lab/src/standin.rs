//! A stand-in for the engine in software: it writes slice data that any
//! decoder accepts, so that everything around the engine can be tested where
//! there is none. A key frame is made of I_PCM macroblocks, which carry the
//! picture's samples as they are; any other frame skips every macroblock,
//! which repeats the frame before it.

use crate::bits::{BitSink, Put};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bits::{BitSource, without_prevention},
        picture::{Picture, Sequence, render},
    };

    #[test]
    fn a_key_frame_carries_the_picture_as_it_is() {
        let mut picture = Picture::new(64, 48);
        render(Sequence::Motion, 3, &mut picture);
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
