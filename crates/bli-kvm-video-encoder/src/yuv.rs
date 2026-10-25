//! JPEG capture frames to planar I420 for the WebRTC encoders.
//!
//! Two decoders can do it. libjpeg-turbo (feature `turbojpeg`) decodes
//! straight to planar YCbCr at the frame's own chroma subsampling, and at half
//! size in the IDCT when asked to halve; chroma rows (4:2:2) or 2x2 blocks
//! (4:4:4) are then averaged to 4:2:0. zune-jpeg (feature `zune`, the
//! fallback) outputs interleaved full-resolution YCbCr, and chroma is averaged
//! over 2x2 blocks; with `halve`, luma is averaged too. Either way a 1080p
//! capture halved becomes 960x540 at a quarter of the encode cost.

#[cfg(not(any(feature = "zune", feature = "turbojpeg")))]
compile_error!("enable feature `turbojpeg` or `zune` for a JPEG decoder");

use thiserror::Error;
#[cfg(feature = "zune")]
use zune_jpeg::{
    JpegDecoder,
    zune_core::{colorspace::ColorSpace, options::DecoderOptions},
};

#[cfg(feature = "turbojpeg")]
use crate::turbo::{Planes, Subsampling, TurboDecoder};

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

/// Which library decodes the capture's JPEG frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JpegBackend {
    #[cfg(feature = "turbojpeg")]
    Turbo,
    #[cfg(feature = "zune")]
    Zune,
}

impl JpegBackend {
    /// libjpeg-turbo when it is built in, else zune-jpeg.
    pub fn preferred() -> Self {
        #[cfg(feature = "turbojpeg")]
        return Self::Turbo;
        #[cfg(not(feature = "turbojpeg"))]
        return Self::Zune;
    }

    /// Every decoder built in, the preferred one first.
    pub fn available() -> Vec<Self> {
        vec![
            #[cfg(feature = "turbojpeg")]
            Self::Turbo,
            #[cfg(feature = "zune")]
            Self::Zune,
        ]
    }

    pub fn name(self) -> &'static str {
        match self {
            #[cfg(feature = "turbojpeg")]
            Self::Turbo => "libjpeg-turbo",
            #[cfg(feature = "zune")]
            Self::Zune => "zune-jpeg",
        }
    }
}

/// Whether libjpeg-turbo uses its fast (less exact) IDCT in the encoder.
pub const FAST_DCT: bool = false;

/// Reusable conversion buffers; one per encoder thread.
pub struct JpegToI420 {
    backend: JpegBackend,
    #[cfg(feature = "zune")]
    ycbcr: Vec<u8>,
    #[cfg(feature = "turbojpeg")]
    turbo: Option<TurboDecoder>,
    #[cfg(feature = "turbojpeg")]
    fast_dct: bool,
    #[cfg(feature = "turbojpeg")]
    chroma: Vec<u8>,
    /// Time spent in JPEG decoding during the last conversion.
    pub last_decode: std::time::Duration,
}

impl std::fmt::Debug for JpegToI420 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JpegToI420")
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

impl Default for JpegToI420 {
    fn default() -> Self {
        Self::with_backend(JpegBackend::preferred(), FAST_DCT)
    }
}

impl JpegToI420 {
    /// A converter on the given decoder; `fast_dct` applies to libjpeg-turbo.
    pub fn with_backend(backend: JpegBackend, fast_dct: bool) -> Self {
        #[cfg(not(feature = "turbojpeg"))]
        let _ = fast_dct;
        Self {
            backend,
            #[cfg(feature = "zune")]
            ycbcr: Vec::new(),
            #[cfg(feature = "turbojpeg")]
            turbo: None,
            #[cfg(feature = "turbojpeg")]
            fast_dct,
            #[cfg(feature = "turbojpeg")]
            chroma: Vec::new(),
            last_decode: std::time::Duration::ZERO,
        }
    }

    pub fn backend(&self) -> JpegBackend {
        self.backend
    }

    pub fn convert(
        &mut self,
        jpeg: &[u8],
        halve: bool,
        out: &mut I420Frame,
    ) -> Result<(), YuvError> {
        // A torn capture frame (no start or end marker) is dropped instead
        // of encoded with grey blocks.
        if jpeg.len() < 4 || jpeg[..2] != [0xff, 0xd8] || jpeg[jpeg.len() - 2..] != [0xff, 0xd9] {
            return Err(YuvError::Decode);
        }
        match self.backend {
            #[cfg(feature = "turbojpeg")]
            JpegBackend::Turbo => self.convert_turbo(jpeg, halve, out),
            #[cfg(feature = "zune")]
            JpegBackend::Zune => self.convert_zune(jpeg, halve, out),
        }
    }

    #[cfg(feature = "turbojpeg")]
    fn convert_turbo(
        &mut self,
        jpeg: &[u8],
        halve: bool,
        out: &mut I420Frame,
    ) -> Result<(), YuvError> {
        let result = self.decode_turbo(jpeg, halve, out);
        if result.is_err() {
            // A damaged frame can leave the decompressor's tables corrupt,
            // and the next intact frame would then fail too ("Bogus Huffman
            // table definition"): start the next frame on a fresh one.
            self.turbo = None;
        }
        result
    }

    #[cfg(feature = "turbojpeg")]
    fn decode_turbo(
        &mut self,
        jpeg: &[u8],
        halve: bool,
        out: &mut I420Frame,
    ) -> Result<(), YuvError> {
        if self.turbo.is_none() {
            self.turbo = Some(TurboDecoder::new(false).ok_or(YuvError::Decode)?);
        }
        let Self {
            turbo,
            chroma,
            fast_dct,
            ..
        } = self;
        let decoder = turbo.as_mut().ok_or(YuvError::Decode)?;
        // The fast IDCT is used at full size only: scaled by half it
        // returned wrong chroma for 4:2:0 frames (libjpeg-turbo 3.2.0).
        if !decoder.set_fast_dct(*fast_dct && !halve) {
            return Err(YuvError::Decode);
        }
        let header = decoder.header(jpeg).ok_or(YuvError::Decode)?;
        let (width, height) = (header.width, header.height);
        let factor = if halve { 4 } else { 2 };
        if width < 16
            || height < 16
            || width > MAX_DIMENSION
            || height > MAX_DIMENSION
            || width % factor != 0
            || height % factor != 0
        {
            return Err(YuvError::Dimensions);
        }
        let denom = if halve { 2 } else { 1 };
        let (out_width, out_height) = (width / denom, height / denom);
        let (chroma_width, chroma_height) = (out_width / 2, out_height / 2);
        out.width = out_width;
        out.height = out_height;
        out.y.resize(out_width * out_height, 0);
        out.u.resize(chroma_width * chroma_height, 0);
        out.v.resize(chroma_width * chroma_height, 0);
        let decode_start = std::time::Instant::now();
        // The decoded chroma planes: the output's own for 4:2:0, a scratch
        // pair of full-height (4:2:2) or full-size (4:4:4) planes otherwise.
        let (source_width, source_height) = match header.subsampling {
            Subsampling::S420 | Subsampling::Gray => (chroma_width, chroma_height),
            Subsampling::S422 => (chroma_width, out_height),
            Subsampling::S444 => (out_width, out_height),
            Subsampling::Other => return Err(YuvError::Decode),
        };
        let decoded = match header.subsampling {
            Subsampling::S420 | Subsampling::Gray => decoder.decode(
                jpeg,
                header,
                denom,
                Planes {
                    y: &mut out.y,
                    y_stride: out_width,
                    u: &mut out.u,
                    u_stride: chroma_width,
                    v: &mut out.v,
                    v_stride: chroma_width,
                },
            ),
            _ => {
                let plane = source_width * source_height;
                chroma.resize(2 * plane, 0);
                let (u, v) = chroma.split_at_mut(plane);
                decoder.decode(
                    jpeg,
                    header,
                    denom,
                    Planes {
                        y: &mut out.y,
                        y_stride: out_width,
                        u,
                        u_stride: source_width,
                        v,
                        v_stride: source_width,
                    },
                )
            }
        };
        self.last_decode = decode_start.elapsed();
        if !decoded {
            return Err(YuvError::Decode);
        }
        match header.subsampling {
            Subsampling::Gray => {
                out.u.fill(128);
                out.v.fill(128);
            }
            Subsampling::S422 => {
                let (u, v) = self.chroma.split_at(source_width * source_height);
                halve_rows(u, source_width, &mut out.u);
                halve_rows(v, source_width, &mut out.v);
            }
            Subsampling::S444 => {
                let (u, v) = self.chroma.split_at(source_width * source_height);
                halve_both(u, source_width, &mut out.u, chroma_width);
                halve_both(v, source_width, &mut out.v, chroma_width);
            }
            Subsampling::S420 | Subsampling::Other => {}
        }
        Ok(())
    }

    #[cfg(feature = "zune")]
    fn convert_zune(
        &mut self,
        jpeg: &[u8],
        halve: bool,
        out: &mut I420Frame,
    ) -> Result<(), YuvError> {
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

/// Averages each pair of rows of a `width`-wide plane into one output row.
#[cfg(feature = "turbojpeg")]
fn halve_rows(source: &[u8], width: usize, out: &mut [u8]) {
    for (row, pair) in out
        .chunks_exact_mut(width)
        .zip(source.chunks_exact(2 * width))
    {
        let (top, bottom) = pair.split_at(width);
        for ((value, &a), &b) in row.iter_mut().zip(top).zip(bottom) {
            *value = (u16::from(a) + u16::from(b)).div_ceil(2) as u8;
        }
    }
}

/// Averages each 2x2 block of a `width`-wide plane into one output sample.
#[cfg(feature = "turbojpeg")]
fn halve_both(source: &[u8], width: usize, out: &mut [u8], out_width: usize) {
    for (row, pair) in out
        .chunks_exact_mut(out_width)
        .zip(source.chunks_exact(2 * width))
    {
        let (top, bottom) = pair.split_at(width);
        for ((value, a), b) in row
            .iter_mut()
            .zip(top.chunks_exact(2))
            .zip(bottom.chunks_exact(2))
        {
            let sum = u16::from(a[0]) + u16::from(a[1]) + u16::from(b[0]) + u16::from(b[1]);
            *value = ((sum + 2) / 4) as u8;
        }
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

    #[cfg(feature = "turbojpeg")]
    const PATTERN_444: &[u8] = include_bytes!("../tests/fixtures/pattern-320x180-444.jpg");
    #[cfg(feature = "turbojpeg")]
    const PATTERN_422: &[u8] = include_bytes!("../tests/fixtures/pattern-320x180-422.jpg");
    #[cfg(feature = "turbojpeg")]
    const PATTERN_420: &[u8] = include_bytes!("../tests/fixtures/pattern-320x180-420.jpg");
    #[cfg(feature = "turbojpeg")]
    const PATTERN_GRAY: &[u8] = include_bytes!("../tests/fixtures/pattern-320x180-gray.jpg");

    #[cfg(feature = "turbojpeg")]
    fn psnr(a: &[u8], b: &[u8]) -> f64 {
        assert_eq!(a.len(), b.len());
        let error: f64 = a
            .iter()
            .zip(b)
            .map(|(&x, &y)| (f64::from(x) - f64::from(y)).powi(2))
            .sum::<f64>()
            / a.len() as f64;
        if error == 0.0 {
            f64::INFINITY
        } else {
            10.0 * (255.0_f64 * 255.0 / error).log10()
        }
    }

    #[test]
    fn full_and_halved_frames_have_consistent_planes() {
        for backend in JpegBackend::available() {
            let mut converter = JpegToI420::with_backend(backend, FAST_DCT);
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
                // Halving preserves the mean brightness and colour.
                assert!((mean(&full.y) - mean(&half.y)).abs() < 1.0, "{backend:?}");
                assert!((mean(&full.u) - mean(&half.u)).abs() < 1.0, "{backend:?}");
                // testsrc2 is colourful: chroma is not flat grey.
                assert!(full.u.iter().any(|&value| value.abs_diff(128) > 20));
            }
        }
    }

    #[test]
    fn invalid_or_truncated_input_is_rejected() {
        for backend in JpegBackend::available() {
            let mut converter = JpegToI420::with_backend(backend, FAST_DCT);
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
    }

    /// libjpeg-turbo's planes match zune-jpeg's conversion: the same frame
    /// within PSNR 45 dB on every plane, for every subsampling the capture
    /// devices use, at full size and halved.
    #[cfg(all(feature = "turbojpeg", feature = "zune"))]
    #[test]
    fn libjpeg_turbo_matches_the_zune_path() {
        for fast_dct in [false, true] {
            let mut turbo = JpegToI420::with_backend(JpegBackend::Turbo, fast_dct);
            let mut zune = JpegToI420::with_backend(JpegBackend::Zune, false);
            for (name, jpeg) in [
                ("444", PATTERN_444),
                ("422", PATTERN_422),
                ("420", PATTERN_420),
                ("testsrc 420", FRAME_420),
                ("testsrc 422", FRAME_422),
            ] {
                for halve in [false, true] {
                    let (mut a, mut b) = (I420Frame::default(), I420Frame::default());
                    turbo.convert(jpeg, halve, &mut a).unwrap();
                    zune.convert(jpeg, halve, &mut b).unwrap();
                    assert_eq!((a.width, a.height), (b.width, b.height), "{name}");
                    // Luma matches the old path within 45 dB everywhere,
                    // chroma where both start from the same samples (4:4:4 at
                    // full size). Elsewhere the old path's chroma is smoothed
                    // by the decoder's upsampling; the source comparison
                    // below holds the new path to at least its accuracy.
                    let chroma_same_source = name == "444" && !halve;
                    for (plane, x, y) in [("y", &a.y, &b.y), ("u", &a.u, &b.u), ("v", &a.v, &b.v)] {
                        let value = psnr(x, y);
                        let floor = if plane == "y" || chroma_same_source {
                            45.0
                        } else {
                            25.0
                        };
                        assert!(
                            value >= floor,
                            "{name} halve={halve} fast_dct={fast_dct} {plane}: {value:.1} dB < {floor}"
                        );
                    }
                }
            }
        }
    }

    /// The synthetic picture the pattern fixtures were made from (cjpeg,
    /// quality 85; tests/fixtures/README.md), as full-range YCbCr.
    #[cfg(all(feature = "turbojpeg", feature = "zune"))]
    fn pattern_ycbcr(x: usize, y: usize) -> (f64, f64, f64) {
        let (width, height) = (320, 180);
        let (mut r, mut g, mut b) = ((x * 255) / (width - 1), (y * 255) / (height - 1), 128);
        if (60..120).contains(&y) {
            const BARS: [(usize, usize, usize); 8] = [
                (255, 255, 255),
                (255, 255, 0),
                (0, 255, 255),
                (0, 255, 0),
                (255, 0, 255),
                (255, 0, 0),
                (0, 0, 255),
                (0, 0, 0),
            ];
            (r, g, b) = BARS[x / 40];
        }
        if y >= 130 && (20..300).contains(&x) {
            (r, g, b) = (235, 235, 235);
            if (y % 8 == 1 || y % 8 == 4) && (x / 3) % 5 != 0 {
                (r, g, b) = (20, 20, 20);
            }
            if x % 7 == 0 && y % 8 < 6 {
                (r, g, b) = (20, 20, 20);
            }
        }
        let (r, g, b) = (r as f64, g as f64, b as f64);
        (
            0.299 * r + 0.587 * g + 0.114 * b,
            128.0 - 0.168_736 * r - 0.331_264 * g + 0.5 * b,
            128.0 + 0.5 * r - 0.418_688 * g - 0.081_312 * b,
        )
    }

    /// The source picture in I420 (chroma averaged over 2x2 blocks).
    #[cfg(all(feature = "turbojpeg", feature = "zune"))]
    fn pattern_truth() -> I420Frame {
        let (width, height) = (320, 180);
        let clamp = |value: f64| value.round().clamp(0.0, 255.0) as u8;
        let mut truth = I420Frame {
            width,
            height,
            y: Vec::with_capacity(width * height),
            u: Vec::with_capacity(width * height / 4),
            v: Vec::with_capacity(width * height / 4),
        };
        for y in 0..height {
            for x in 0..width {
                truth.y.push(clamp(pattern_ycbcr(x, y).0));
            }
        }
        for y in 0..height / 2 {
            for x in 0..width / 2 {
                let (mut cb, mut cr) = (0.0, 0.0);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (_, b, r) = pattern_ycbcr(2 * x + dx, 2 * y + dy);
                    cb += b / 4.0;
                    cr += r / 4.0;
                }
                truth.u.push(clamp(cb));
                truth.v.push(clamp(cr));
            }
        }
        truth
    }

    /// Against the picture the fixtures were made from, libjpeg-turbo's
    /// frame is at least as close as the zune path's in every plane: its
    /// chroma comes from the stored samples, while the zune path's chroma
    /// was first upsampled by the decoder and so is smoothed.
    #[cfg(all(feature = "turbojpeg", feature = "zune"))]
    #[test]
    fn libjpeg_turbo_is_at_least_as_close_to_the_source() {
        let truth = pattern_truth();
        let mut turbo = JpegToI420::with_backend(JpegBackend::Turbo, false);
        let mut zune = JpegToI420::with_backend(JpegBackend::Zune, false);
        for (name, jpeg) in [
            ("444", PATTERN_444),
            ("422", PATTERN_422),
            ("420", PATTERN_420),
        ] {
            let (mut a, mut b) = (I420Frame::default(), I420Frame::default());
            turbo.convert(jpeg, false, &mut a).unwrap();
            zune.convert(jpeg, false, &mut b).unwrap();
            for (plane, new, old, source) in [
                ("y", &a.y, &b.y, &truth.y),
                ("u", &a.u, &b.u, &truth.u),
                ("v", &a.v, &b.v, &truth.v),
            ] {
                let (new, old) = (psnr(new, source), psnr(old, source));
                eprintln!("{name} {plane}: libjpeg-turbo {new:.1} dB, zune {old:.1} dB");
                assert!(new >= old - 0.5, "{name} {plane}: {new:.1} < {old:.1} dB");
            }
        }
    }

    #[cfg(feature = "turbojpeg")]
    #[test]
    fn grayscale_frames_get_neutral_chroma() {
        let mut converter = JpegToI420::with_backend(JpegBackend::Turbo, FAST_DCT);
        let mut out = I420Frame::default();
        converter.convert(PATTERN_GRAY, false, &mut out).unwrap();
        assert_eq!((out.width, out.height), (320, 180));
        assert!(out.u.iter().chain(&out.v).all(|&value| value == 128));
        let mut colour = I420Frame::default();
        converter.convert(PATTERN_422, false, &mut colour).unwrap();
        assert!(psnr(&out.y, &colour.y) > 30.0);
    }

    /// Randomly damaged frames are refused or decoded, never a crash. The
    /// count comes from BK_JPEG_DAMAGE_FRAMES (check.sh runs 20000 in a
    /// release build).
    #[cfg(feature = "turbojpeg")]
    #[test]
    fn damaged_frames_never_crash_libjpeg_turbo() {
        let frames: usize = std::env::var("BK_JPEG_DAMAGE_FRAMES")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(500);
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let fixtures = [
            PATTERN_444,
            PATTERN_422,
            PATTERN_420,
            PATTERN_GRAY,
            FRAME_420,
            FRAME_422,
        ];
        let mut converter = JpegToI420::with_backend(JpegBackend::Turbo, FAST_DCT);
        let mut out = I420Frame::default();
        let (mut decoded, mut refused) = (0_usize, 0_usize);
        for _ in 0..frames {
            let source = fixtures[(next() % fixtures.len() as u64) as usize];
            let mut jpeg = source.to_vec();
            for _ in 0..1 + next() % 8 {
                let at = (next() % jpeg.len() as u64) as usize;
                match next() % 5 {
                    0 => jpeg[at] ^= 1 << (next() % 8),
                    1 => jpeg[at] = next() as u8,
                    2 => jpeg[at] = 0xff,
                    3 => {
                        // Cut the frame but keep its end marker, so the
                        // marker check lets it through to the decoder.
                        jpeg.truncate(at.max(4));
                        jpeg.extend_from_slice(&[0xff, 0xd9]);
                    }
                    _ => {
                        let length = (next() % 64) as usize;
                        let from = (next() % jpeg.len() as u64) as usize;
                        let chunk: Vec<u8> = jpeg[from..(from + length).min(jpeg.len())].to_vec();
                        let at = at.min(jpeg.len());
                        jpeg.splice(at..at, chunk);
                    }
                }
            }
            let halve = next() % 2 == 0;
            match converter.convert(&jpeg, halve, &mut out) {
                Ok(()) => {
                    decoded += 1;
                    assert_eq!(out.y.len(), out.width * out.height);
                    assert_eq!(out.u.len(), out.width / 2 * (out.height / 2));
                    assert_eq!(out.v.len(), out.u.len());
                }
                Err(_) => refused += 1,
            }
        }
        assert_eq!(decoded + refused, frames);
        eprintln!("damaged frames: {decoded} decoded, {refused} refused");
        // Both outcomes are exercised: some damage leaves a decodable frame.
        assert!(frames < 100 || (decoded > 0 && refused > 0));
        // The same converter still decodes an intact frame afterwards.
        let result = converter.convert(PATTERN_422, false, &mut out);
        assert!(
            result.is_ok(),
            "{result:?}: {}",
            converter
                .turbo
                .as_ref()
                .map(|turbo| turbo.last_error())
                .unwrap_or_default()
        );
        assert_eq!((out.width, out.height), (320, 180));
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
