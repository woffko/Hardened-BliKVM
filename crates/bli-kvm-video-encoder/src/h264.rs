//! Safe wrapper over the narrow x264 realtime H.264 encoder shim.

use std::ffi::c_void;

use thiserror::Error;

use crate::vp8::{EncodedFrame, I420};

#[repr(C)]
struct RawEncoder {
    _private: [u8; 0],
}

type Emit = unsafe extern "C" fn(*mut c_void, *const u8, usize, i32);

unsafe extern "C" {
    #[allow(clippy::too_many_arguments)]
    fn bk_h264_open(
        width: u32,
        height: u32,
        fps: u32,
        kbps: u32,
        threads: u32,
        keyframe_interval: u32,
        preset: u32,
        out: *mut *mut RawEncoder,
    ) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn bk_h264_encode(
        encoder: *mut RawEncoder,
        y: *const u8,
        u: *const u8,
        v: *const u8,
        y_stride: u32,
        uv_stride: u32,
        pts_us: i64,
        force_key: i32,
        emit: Emit,
        context: *mut c_void,
    ) -> i32;
    fn bk_h264_set_bitrate(encoder: *mut RawEncoder, kbps: u32) -> i32;
    fn bk_h264_close(encoder: *mut RawEncoder);
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum H264Error {
    #[error("encoder configuration is invalid")]
    InvalidConfig,
    #[error("frame planes do not match the configured size")]
    InvalidFrame,
    #[error("x264 reported error {0}")]
    Codec(i32),
}

/// x264's speed presets that keep up in real time on the H616.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H264Preset {
    Superfast,
    Ultrafast,
}

impl H264Preset {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "superfast" => Some(Self::Superfast),
            "ultrafast" => Some(Self::Ultrafast),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct H264Config {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub kbps: u32,
    pub threads: u32,
    pub keyframe_interval: u32,
    pub preset: H264Preset,
}

pub struct H264Encoder {
    raw: *mut RawEncoder,
    config: H264Config,
}

// The encoder is used from one thread at a time; x264 keeps no thread-local
// state tied to the creating thread.
unsafe impl Send for H264Encoder {}

unsafe extern "C" fn collect(context: *mut c_void, data: *const u8, size: usize, key: i32) {
    // SAFETY: context is the &mut Vec passed to bk_h264_encode for this call
    // only; x264 guarantees data points to `size` readable bytes.
    let frames = unsafe { &mut *(context as *mut Vec<EncodedFrame>) };
    let bytes = unsafe { std::slice::from_raw_parts(data, size) };
    frames.push(EncodedFrame {
        data: bytes.to_vec(),
        key_frame: key != 0,
    });
}

impl H264Encoder {
    pub fn new(config: H264Config) -> Result<Self, H264Error> {
        let mut raw = std::ptr::null_mut();
        let preset = match config.preset {
            H264Preset::Superfast => 0,
            H264Preset::Ultrafast => 1,
        };
        // SAFETY: the shim validates every value and only writes `raw` on success.
        let code = unsafe {
            bk_h264_open(
                config.width,
                config.height,
                config.fps,
                config.kbps,
                config.threads,
                config.keyframe_interval,
                preset,
                &mut raw,
            )
        };
        match code {
            0 if !raw.is_null() => Ok(Self { raw, config }),
            -1 => Err(H264Error::InvalidConfig),
            code => Err(H264Error::Codec(code)),
        }
    }

    pub fn config(&self) -> H264Config {
        self.config
    }

    /// Changes the target bitrate from the next frame on.
    pub fn set_bitrate(&mut self, kbps: u32) -> Result<(), H264Error> {
        // SAFETY: raw is a live encoder owned by self.
        match unsafe { bk_h264_set_bitrate(self.raw, kbps) } {
            0 => {
                self.config.kbps = kbps;
                Ok(())
            }
            -1 => Err(H264Error::InvalidConfig),
            code => Err(H264Error::Codec(code)),
        }
    }

    /// Encode one frame; returns zero or one access unit (Annex B).
    pub fn encode(
        &mut self,
        frame: I420<'_>,
        pts_us: i64,
        force_key: bool,
    ) -> Result<Vec<EncodedFrame>, H264Error> {
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
        {
            return Err(H264Error::InvalidFrame);
        }
        let mut frames: Vec<EncodedFrame> = Vec::new();
        // SAFETY: plane bounds were checked above for the configured size;
        // the callback context outlives the call and is not aliased.
        let code = unsafe {
            bk_h264_encode(
                self.raw,
                frame.y.as_ptr(),
                frame.u.as_ptr(),
                frame.v.as_ptr(),
                frame.y_stride as u32,
                frame.uv_stride as u32,
                pts_us,
                i32::from(force_key),
                collect,
                &mut frames as *mut Vec<EncodedFrame> as *mut c_void,
            )
        };
        if code != 0 {
            return Err(H264Error::Codec(code));
        }
        Ok(frames)
    }
}

impl Drop for H264Encoder {
    fn drop(&mut self) {
        // SAFETY: raw came from bk_h264_open and is released exactly once.
        unsafe { bk_h264_close(self.raw) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(preset: H264Preset) -> H264Config {
        H264Config {
            width: 64,
            height: 48,
            fps: 30,
            kbps: 300,
            threads: 2,
            keyframe_interval: 60,
            preset,
        }
    }

    #[test]
    fn invalid_configurations_are_refused_by_the_shim() {
        let base = config(H264Preset::Superfast);
        for broken in [
            H264Config { width: 63, ..base },
            H264Config { height: 8, ..base },
            H264Config { fps: 0, ..base },
            H264Config { kbps: 10, ..base },
            H264Config { threads: 9, ..base },
            H264Config {
                keyframe_interval: 0,
                ..base
            },
        ] {
            assert_eq!(
                H264Encoder::new(broken).err(),
                Some(H264Error::InvalidConfig)
            );
        }
        assert_eq!(H264Preset::parse("superfast"), Some(H264Preset::Superfast));
        assert_eq!(H264Preset::parse("ultrafast"), Some(H264Preset::Ultrafast));
        assert_eq!(H264Preset::parse("medium"), None);
    }

    #[test]
    fn frames_encode_with_headers_on_key_frames() {
        for preset in [H264Preset::Superfast, H264Preset::Ultrafast] {
            let mut encoder = H264Encoder::new(config(preset)).unwrap();
            let mut y = vec![16_u8; 64 * 48];
            let u = vec![128_u8; 32 * 24];
            let v = vec![128_u8; 32 * 24];
            let mut keys = 0;
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
                        index == 5,
                    )
                    .unwrap();
                for frame in frames {
                    // Annex B with a start code; a key frame carries its SPS
                    // (NAL type 7) for viewers that join late.
                    assert!(
                        frame.data.starts_with(&[0, 0, 0, 1]) || frame.data.starts_with(&[0, 0, 1])
                    );
                    if frame.key_frame {
                        keys += 1;
                        assert!(
                            frame
                                .data
                                .windows(5)
                                .any(|w| w[..4] == [0, 0, 0, 1] && w[4] & 0x1f == 7)
                        );
                    }
                }
            }
            // The first frame and the forced one.
            assert_eq!(keys, 2, "{preset:?}");
        }
    }

    #[test]
    fn a_1080p_stream_matches_the_profile_janus_announces() {
        // Janus offers profile-level-id=42e028: constrained baseline (66 with
        // constraint_set1), level 4.0 or lower.
        let mut encoder = H264Encoder::new(H264Config {
            width: 1920,
            height: 1080,
            fps: 30,
            kbps: 2500,
            threads: 2,
            keyframe_interval: 60,
            preset: H264Preset::Ultrafast,
        })
        .unwrap();
        let y: Vec<u8> = (0..1920 * 1080).map(|value| (value % 251) as u8).collect();
        let (u, v) = (vec![128_u8; 960 * 540], vec![128_u8; 960 * 540]);
        let frames = encoder
            .encode(
                I420 {
                    y: &y,
                    u: &u,
                    v: &v,
                    y_stride: 1920,
                    uv_stride: 960,
                },
                0,
                true,
            )
            .unwrap();
        let data = &frames[0].data;
        let sps = data
            .windows(4)
            .position(|w| w[..3] == [0, 0, 1] && w[3] & 0x1f == 7)
            .expect("an SPS")
            + 3;
        let (profile, constraints, level) = (data[sps + 1], data[sps + 2], data[sps + 3]);
        assert_eq!(profile, 66, "baseline");
        assert_ne!(
            constraints & 0x40,
            0,
            "constraint_set1: constrained baseline"
        );
        assert!(level <= 40, "level {level}");
    }
}
