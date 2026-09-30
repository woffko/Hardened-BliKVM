//! Safe wrapper over the narrow libvpx VP8 realtime encoder shim.

use std::ffi::c_void;

use thiserror::Error;

#[repr(C)]
struct RawEncoder {
    _private: [u8; 0],
}

type Emit = unsafe extern "C" fn(*mut c_void, *const u8, usize, i32);

unsafe extern "C" {
    fn bk_vp8_open(
        width: u32,
        height: u32,
        fps: u32,
        kbps: u32,
        threads: u32,
        keyframe_interval: u32,
        out: *mut *mut RawEncoder,
    ) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn bk_vp8_encode(
        encoder: *mut RawEncoder,
        y: *const u8,
        u: *const u8,
        v: *const u8,
        y_stride: u32,
        uv_stride: u32,
        pts_us: i64,
        duration_us: i64,
        force_key: i32,
        emit: Emit,
        context: *mut c_void,
    ) -> i32;
    fn bk_vp8_set_bitrate(encoder: *mut RawEncoder, kbps: u32) -> i32;
    fn bk_vp8_close(encoder: *mut RawEncoder);
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Vp8Error {
    #[error("encoder configuration is invalid")]
    InvalidConfig,
    #[error("frame planes do not match the configured size")]
    InvalidFrame,
    #[error("libvpx reported error {0}")]
    Codec(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vp8Config {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub kbps: u32,
    pub threads: u32,
    pub keyframe_interval: u32,
}

/// One I420 frame borrowed for a single encode call.
#[derive(Debug, Clone, Copy)]
pub struct I420<'a> {
    pub y: &'a [u8],
    pub u: &'a [u8],
    pub v: &'a [u8],
    pub y_stride: usize,
    pub uv_stride: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub key_frame: bool,
}

pub struct Vp8Encoder {
    raw: *mut RawEncoder,
    config: Vp8Config,
}

// The encoder is used from one thread at a time; libvpx keeps no thread-local
// state tied to the creating thread.
unsafe impl Send for Vp8Encoder {}

unsafe extern "C" fn collect(context: *mut c_void, data: *const u8, size: usize, key: i32) {
    // SAFETY: context is the &mut Vec passed to bk_vp8_encode for this call
    // only; libvpx guarantees data points to `size` readable bytes.
    let frames = unsafe { &mut *(context as *mut Vec<EncodedFrame>) };
    let bytes = unsafe { std::slice::from_raw_parts(data, size) };
    frames.push(EncodedFrame {
        data: bytes.to_vec(),
        key_frame: key != 0,
    });
}

impl Vp8Encoder {
    pub fn new(config: Vp8Config) -> Result<Self, Vp8Error> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: the shim validates every value and only writes `raw` on success.
        let code = unsafe {
            bk_vp8_open(
                config.width,
                config.height,
                config.fps,
                config.kbps,
                config.threads,
                config.keyframe_interval,
                &mut raw,
            )
        };
        match code {
            0 if !raw.is_null() => Ok(Self { raw, config }),
            -1 => Err(Vp8Error::InvalidConfig),
            code => Err(Vp8Error::Codec(code)),
        }
    }

    pub fn config(&self) -> Vp8Config {
        self.config
    }

    /// Encode one frame; returns zero or more compressed frames.
    pub fn encode(
        &mut self,
        frame: I420<'_>,
        pts_us: i64,
        duration_us: i64,
        force_key: bool,
    ) -> Result<Vec<EncodedFrame>, Vp8Error> {
        let width = self.config.width as usize;
        let height = self.config.height as usize;
        let chroma_width = width.div_ceil(2);
        let chroma_height = height.div_ceil(2);
        let fits = |plane: &[u8], stride: usize, row: usize, rows: usize| {
            stride >= row
                && stride <= u32::MAX as usize
                && stride
                    .checked_mul(rows - 1)
                    .and_then(|bytes| bytes.checked_add(row))
                    .is_some_and(|needed| plane.len() >= needed)
        };
        if !fits(frame.y, frame.y_stride, width, height)
            || !fits(frame.u, frame.uv_stride, chroma_width, chroma_height)
            || !fits(frame.v, frame.uv_stride, chroma_width, chroma_height)
            || pts_us < 0
            || duration_us <= 0
        {
            return Err(Vp8Error::InvalidFrame);
        }
        let mut frames: Vec<EncodedFrame> = Vec::new();
        // SAFETY: plane bounds were checked above for the configured size;
        // the callback context outlives the call and is not aliased.
        let code = unsafe {
            bk_vp8_encode(
                self.raw,
                frame.y.as_ptr(),
                frame.u.as_ptr(),
                frame.v.as_ptr(),
                frame.y_stride as u32,
                frame.uv_stride as u32,
                pts_us,
                duration_us,
                i32::from(force_key),
                collect,
                &mut frames as *mut Vec<EncodedFrame> as *mut c_void,
            )
        };
        if code != 0 {
            return Err(Vp8Error::Codec(code));
        }
        Ok(frames)
    }

    pub fn set_bitrate(&mut self, kbps: u32) -> Result<(), Vp8Error> {
        // SAFETY: raw is a live encoder owned by self.
        match unsafe { bk_vp8_set_bitrate(self.raw, kbps) } {
            0 => {
                self.config.kbps = kbps;
                Ok(())
            }
            -1 => Err(Vp8Error::InvalidConfig),
            code => Err(Vp8Error::Codec(code)),
        }
    }
}

impl Drop for Vp8Encoder {
    fn drop(&mut self) {
        // SAFETY: raw came from bk_vp8_open and is released exactly once.
        unsafe { bk_vp8_close(self.raw) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Vp8Config {
        Vp8Config {
            width: 64,
            height: 48,
            fps: 30,
            kbps: 300,
            threads: 2,
            keyframe_interval: 60,
        }
    }

    #[test]
    fn invalid_configurations_are_refused_by_the_shim() {
        for broken in [
            Vp8Config {
                width: 63,
                ..config()
            },
            Vp8Config {
                height: 8,
                ..config()
            },
            Vp8Config { fps: 0, ..config() },
            Vp8Config {
                kbps: 10,
                ..config()
            },
            Vp8Config {
                threads: 9,
                ..config()
            },
            Vp8Config {
                keyframe_interval: 0,
                ..config()
            },
        ] {
            assert_eq!(Vp8Encoder::new(broken).err(), Some(Vp8Error::InvalidConfig));
        }
    }

    #[test]
    fn frames_encode_with_a_leading_key_frame_and_forced_key_frames() {
        let mut encoder = Vp8Encoder::new(config()).unwrap();
        let mut y = vec![16_u8; 64 * 48];
        let u = vec![128_u8; 32 * 24];
        let v = vec![128_u8; 32 * 24];
        let mut total = 0;
        for index in 0..10_i64 {
            let spot = (index as usize * 97) % y.len();
            y[spot] = 235;
            let frames = encoder
                .encode(
                    I420 {
                        y: &y,
                        u: &u,
                        v: &v,
                        y_stride: 64,
                        uv_stride: 32,
                    },
                    index * 33_333,
                    33_333,
                    index == 5,
                )
                .unwrap();
            for frame in &frames {
                assert!(!frame.data.is_empty());
                // VP8 frame tag: bit 0 clear marks a key frame.
                assert_eq!(frame.data[0] & 1 == 0, frame.key_frame);
                if index == 0 || index == 5 {
                    assert!(frame.key_frame);
                }
            }
            total += frames.len();
        }
        assert!(total >= 2);
        encoder.set_bitrate(500).unwrap();
        assert_eq!(encoder.config().kbps, 500);
        assert_eq!(encoder.set_bitrate(1), Err(Vp8Error::InvalidConfig));
    }

    #[test]
    fn short_planes_are_rejected_before_the_codec() {
        let mut encoder = Vp8Encoder::new(config()).unwrap();
        let y = vec![0_u8; 64 * 48 - 1];
        let chroma = vec![0_u8; 32 * 24];
        assert_eq!(
            encoder
                .encode(
                    I420 {
                        y: &y,
                        u: &chroma,
                        v: &chroma,
                        y_stride: 64,
                        uv_stride: 32,
                    },
                    0,
                    1,
                    false
                )
                .err(),
            Some(Vp8Error::InvalidFrame)
        );
    }
}
