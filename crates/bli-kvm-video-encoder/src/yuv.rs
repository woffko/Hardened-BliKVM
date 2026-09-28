//! JPEG capture frames to planar I420 for the VP8 encoder.
//!
//! The decoder outputs interleaved full-resolution YCbCr; chroma is averaged
//! over 2x2 blocks. With `halve`, luma is averaged too, which turns a 1080p
//! capture into 960x540 at a quarter of the encode cost.

use thiserror::Error;
use zune_jpeg::{
    JpegDecoder,
    zune_core::{colorspace::ColorSpace, options::DecoderOptions},
};

/// Bounds any capture frame the decoder will accept.
pub const MAX_DIMENSION: usize = 4096;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum YuvError {
    #[error("JPEG frame could not be decoded")]
    Decode,
    #[error("JPEG frame has unsupported dimensions")]
    Dimensions,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct I420Frame {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

/// Reusable conversion buffers; one per encoder thread.
#[derive(Debug, Default)]
pub struct JpegToI420 {
    ycbcr: Vec<u8>,
    /// Time spent in JPEG entropy decoding during the last conversion.
    pub last_decode: std::time::Duration,
}

impl JpegToI420 {
    pub fn convert(
        &mut self,
        jpeg: &[u8],
        halve: bool,
        out: &mut I420Frame,
    ) -> Result<(), YuvError> {
        // The decoder conceals truncated scans; a torn capture frame must be
        // dropped instead of encoded as grey blocks.
        if jpeg.len() < 4 || jpeg[..2] != [0xff, 0xd8] || jpeg[jpeg.len() - 2..] != [0xff, 0xd9] {
            return Err(YuvError::Decode);
        }
        let options = DecoderOptions::default()
            .set_strict_mode(true)
            .set_max_width(MAX_DIMENSION)
            .set_max_height(MAX_DIMENSION)
            .jpeg_set_out_colorspace(ColorSpace::YCbCr);
        let mut decoder = JpegDecoder::new_with_options(jpeg, options);
        decoder.decode_headers().map_err(|_| YuvError::Decode)?;
        let info = decoder.info().ok_or(YuvError::Decode)?;
        let (width, height) = (usize::from(info.width), usize::from(info.height));
        let factor = if halve { 4 } else { 2 };
        if width < 16 || height < 16 || width % factor != 0 || height % factor != 0 {
            return Err(YuvError::Dimensions);
        }
        let size = decoder.output_buffer_size().ok_or(YuvError::Decode)?;
        if size != width * height * 3 {
            return Err(YuvError::Decode);
        }
        self.ycbcr.resize(size, 0);
        let decode_start = std::time::Instant::now();
        let decoded = decoder.decode_into(&mut self.ycbcr);
        self.last_decode = decode_start.elapsed();
        decoded.map_err(|_| YuvError::Decode)?;
        let (out_width, out_height) = if halve {
            (width / 2, height / 2)
        } else {
            (width, height)
        };
        out.width = out_width;
        out.height = out_height;
        out.y.resize(out_width * out_height, 0);
        out.u.resize(out_width / 2 * (out_height / 2), 0);
        out.v.resize(out_width / 2 * (out_height / 2), 0);
        let row_bytes = width * 3;
        let rows = |y: usize| &self.ycbcr[y * row_bytes..(y + 1) * row_bytes];
        if halve {
            for (y, luma) in out.y.chunks_exact_mut(out_width).enumerate() {
                let (top, bottom) = (rows(2 * y), rows(2 * y + 1));
                for ((value, a), b) in luma
                    .iter_mut()
                    .zip(top.chunks_exact(6))
                    .zip(bottom.chunks_exact(6))
                {
                    let sum = u16::from(a[0]) + u16::from(a[3]) + u16::from(b[0]) + u16::from(b[3]);
                    *value = ((sum + 2) / 4) as u8;
                }
            }
        } else {
            for (luma, pixel) in out.y.iter_mut().zip(self.ycbcr.chunks_exact(3)) {
                *luma = pixel[0];
            }
        }
        // Each output chroma sample covers a 2x2 output block: a 2x2 source
        // block at full size, or the four source samples at the centre of the
        // 4x4 block when halving, which keeps the cost at four reads.
        let (step, offset) = if halve { (4, 1) } else { (2, 0) };
        let chroma_width = out_width / 2;
        for (y, (u_row, v_row)) in out
            .u
            .chunks_exact_mut(chroma_width)
            .zip(out.v.chunks_exact_mut(chroma_width))
            .enumerate()
        {
            let (top, bottom) = (rows(y * step + offset), rows(y * step + offset + 1));
            for x in 0..chroma_width {
                let base = (x * step + offset) * 3;
                let cb = u16::from(top[base + 1])
                    + u16::from(top[base + 4])
                    + u16::from(bottom[base + 1])
                    + u16::from(bottom[base + 4]);
                let cr = u16::from(top[base + 2])
                    + u16::from(top[base + 5])
                    + u16::from(bottom[base + 2])
                    + u16::from(bottom[base + 5]);
                u_row[x] = ((cb + 2) / 4) as u8;
                v_row[x] = ((cr + 2) / 4) as u8;
            }
        }
        Ok(())
    }
}

/// Scales an I420 frame to two thirds of its size (1920x1080 to 1280x720):
/// every three source samples give two, weighted 2:1 and 1:2, across and
/// down. The width and height must be multiples of six.
pub fn scale_two_thirds(src: &I420Frame, dst: &mut I420Frame) -> Result<(), YuvError> {
    if src.width < 12 || src.height < 12 || src.width % 6 != 0 || src.height % 6 != 0 {
        return Err(YuvError::Dimensions);
    }
    let (width, height) = (src.width * 2 / 3, src.height * 2 / 3);
    dst.width = width;
    dst.height = height;
    dst.y.resize(width * height, 0);
    dst.u.resize(width / 2 * (height / 2), 0);
    dst.v.resize(width / 2 * (height / 2), 0);
    plane_two_thirds(&src.y, src.width, src.height, &mut dst.y);
    plane_two_thirds(&src.u, src.width / 2, src.height / 2, &mut dst.u);
    plane_two_thirds(&src.v, src.width / 2, src.height / 2, &mut dst.v);
    Ok(())
}

fn plane_two_thirds(src: &[u8], width: usize, height: usize, dst: &mut [u8]) {
    let out_width = width * 2 / 3;
    let mut rows = [
        vec![0_u16; out_width],
        vec![0_u16; out_width],
        vec![0_u16; out_width],
    ];
    let across = |row: &[u8], out: &mut [u16]| {
        for (pair, triple) in out.chunks_exact_mut(2).zip(row.chunks_exact(3)) {
            let (a, b, c) = (
                u16::from(triple[0]),
                u16::from(triple[1]),
                u16::from(triple[2]),
            );
            pair[0] = 2 * a + b;
            pair[1] = b + 2 * c;
        }
    };
    for (band, out) in dst.chunks_exact_mut(2 * out_width).enumerate() {
        for (index, row) in rows.iter_mut().enumerate() {
            let y = band * 3 + index;
            across(&src[y * width..(y + 1) * width], row);
        }
        let (top, bottom) = out.split_at_mut(out_width);
        for x in 0..out_width {
            // Each value is three times a sample; nine times after both passes.
            top[x] = ((2 * rows[0][x] + rows[1][x] + 4) / 9) as u8;
            bottom[x] = ((rows[1][x] + 2 * rows[2][x] + 4) / 9) as u8;
        }
    }
    let _ = height;
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME_420: &[u8] = include_bytes!("../tests/fixtures/testsrc-64x48-420.jpg");
    const FRAME_422: &[u8] = include_bytes!("../tests/fixtures/testsrc-64x48-422.jpg");

    fn mean(plane: &[u8]) -> f64 {
        plane.iter().map(|&value| f64::from(value)).sum::<f64>() / plane.len() as f64
    }

    #[test]
    fn full_and_halved_frames_have_consistent_planes() {
        let mut converter = JpegToI420::default();
        for jpeg in [FRAME_420, FRAME_422] {
            let mut full = I420Frame::default();
            converter.convert(jpeg, false, &mut full).unwrap();
            assert_eq!((full.width, full.height), (64, 48));
            assert_eq!(full.y.len(), 64 * 48);
            assert_eq!((full.u.len(), full.v.len()), (32 * 24, 32 * 24));
            let mut half = I420Frame::default();
            converter.convert(jpeg, true, &mut half).unwrap();
            assert_eq!((half.width, half.height), (32, 24));
            assert_eq!((half.u.len(), half.v.len()), (16 * 12, 16 * 12));
            // Averaging preserves the mean brightness and colour.
            assert!((mean(&full.y) - mean(&half.y)).abs() < 1.0);
            assert!((mean(&full.u) - mean(&half.u)).abs() < 1.0);
            // testsrc2 is colourful: chroma is not flat grey.
            assert!(full.u.iter().any(|&value| value.abs_diff(128) > 20));
        }
    }

    #[test]
    fn invalid_or_truncated_input_is_rejected() {
        let mut converter = JpegToI420::default();
        let mut out = I420Frame::default();
        assert!(converter.convert(b"not a jpeg", false, &mut out).is_err());
        assert!(
            converter
                .convert(&FRAME_420[..FRAME_420.len() / 3], false, &mut out)
                .is_err()
        );
        // 64x48 is divisible by 2 and 4; 48/4 == 12, so halving is allowed.
        assert!(converter.convert(FRAME_420, true, &mut out).is_ok());
    }

    #[test]
    fn two_thirds_keeps_flat_areas_and_blends_edges() {
        let mut src = I420Frame {
            width: 18,
            height: 12,
            y: vec![90; 18 * 12],
            u: vec![128; 9 * 6],
            v: vec![60; 9 * 6],
        };
        // A bright column at x=1 of every luma row.
        for row in src.y.chunks_exact_mut(18) {
            row[1] = 180;
        }
        let mut dst = I420Frame::default();
        scale_two_thirds(&src, &mut dst).unwrap();
        assert_eq!((dst.width, dst.height), (12, 8));
        assert_eq!(dst.u, vec![128; 6 * 4]);
        assert_eq!(dst.v, vec![60; 6 * 4]);
        // (2*90 + 180)/3 = 120 and (180 + 2*90)/3 = 120, then flat vertically.
        assert!(
            dst.y
                .chunks_exact(12)
                .all(|row| row[0] == 120 && row[1] == 120 && row[2] == 90)
        );
        let odd = I420Frame {
            width: 16,
            ..src.clone()
        };
        assert_eq!(scale_two_thirds(&odd, &mut dst), Err(YuvError::Dimensions));
    }
}
