//! One WebRTC video codec with its RTP packetizer: VP8 (libvpx) or H.264
//! (x264), chosen per encoder instance.

use crate::{
    H264Packetizer, Vp8Packetizer,
    h264::{H264Config, H264Encoder, H264Preset},
    rtp::RtpError,
    vp8::{I420, Vp8Config, Vp8Encoder},
    yuv::I420Frame,
};

/// RTP payload types; each codec has its own Janus mountpoint.
pub const VP8_PAYLOAD_TYPE: u8 = 96;
pub const H264_PAYLOAD_TYPE: u8 = 97;
pub const MTU: usize = 1200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Vp8,
    H264(H264Preset),
}

impl Codec {
    /// `vp8`, `h264` (superfast) or `h264-ultrafast`.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "vp8" => Some(Self::Vp8),
            "h264" | "h264-superfast" => Some(Self::H264(H264Preset::Superfast)),
            "h264-ultrafast" => Some(Self::H264(H264Preset::Ultrafast)),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Vp8 => "vp8",
            Self::H264(H264Preset::Superfast) => "h264-superfast",
            Self::H264(H264Preset::Ultrafast) => "h264-ultrafast",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EngineSettings {
    pub codec: Codec,
    pub fps: u32,
    pub kbps: u32,
    pub threads: u32,
}

enum Encoder {
    Vp8(Vp8Encoder, Vp8Packetizer),
    H264(H264Encoder, H264Packetizer),
}

/// Encodes frames of one size and packetizes them.
pub struct Engine {
    encoder: Encoder,
    width: usize,
    height: usize,
    fps: u32,
}

impl Engine {
    /// `ssrc` and `sequence` should be random per session.
    pub fn new(
        settings: EngineSettings,
        width: usize,
        height: usize,
        ssrc: u32,
        sequence: u16,
    ) -> Result<Self, String> {
        let (w, h) = (width as u32, height as u32);
        // Janus RTP input cannot request key frames; one every two seconds
        // lets joining viewers converge quickly.
        let keyframe_interval = settings.fps * 2;
        let encoder = match settings.codec {
            Codec::Vp8 => Encoder::Vp8(
                Vp8Encoder::new(Vp8Config {
                    width: w,
                    height: h,
                    fps: settings.fps,
                    kbps: settings.kbps,
                    threads: settings.threads,
                    keyframe_interval,
                })
                .map_err(|error| error.to_string())?,
                Vp8Packetizer::new(VP8_PAYLOAD_TYPE, ssrc, sequence, MTU)
                    .map_err(|error| error.to_string())?,
            ),
            Codec::H264(preset) => Encoder::H264(
                H264Encoder::new(H264Config {
                    width: w,
                    height: h,
                    fps: settings.fps,
                    kbps: settings.kbps,
                    threads: settings.threads,
                    keyframe_interval,
                    preset,
                })
                .map_err(|error| error.to_string())?,
                H264Packetizer::new(H264_PAYLOAD_TYPE, ssrc, sequence, MTU)
                    .map_err(|error| error.to_string())?,
            ),
        };
        Ok(Self {
            encoder,
            width,
            height,
            fps: settings.fps,
        })
    }

    /// Changes the target bitrate of the running encoder; the stream goes on.
    pub fn set_bitrate(&mut self, kbps: u32) -> Result<(), String> {
        match &mut self.encoder {
            Encoder::Vp8(encoder, _) => {
                encoder.set_bitrate(kbps).map_err(|error| error.to_string())
            }
            Encoder::H264(encoder, _) => {
                encoder.set_bitrate(kbps).map_err(|error| error.to_string())
            }
        }
    }

    /// The sequence number the next RTP packet carries.
    pub fn next_sequence(&self) -> u16 {
        match &self.encoder {
            Encoder::Vp8(_, packetizer) => packetizer.next_sequence(),
            Encoder::H264(_, packetizer) => packetizer.next_sequence(),
        }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// Encodes one frame taken at `pts_us` and returns its RTP packets.
    pub fn encode(
        &mut self,
        frame: &I420Frame,
        pts_us: i64,
        force_key: bool,
    ) -> Result<Vec<Vec<u8>>, String> {
        let planes = I420 {
            y: &frame.y,
            u: &frame.u,
            v: &frame.v,
            y_stride: frame.width,
            uv_stride: frame.width / 2,
        };
        // Both codecs run the 90 kHz RTP video clock.
        let timestamp = Vp8Packetizer::timestamp(pts_us as u64);
        let mut packets = Vec::new();
        let mut add = |result: Result<Vec<Vec<u8>>, RtpError>| {
            if let Ok(mut more) = result {
                packets.append(&mut more);
            }
        };
        match &mut self.encoder {
            Encoder::Vp8(encoder, packetizer) => {
                let duration = 1_000_000 / i64::from(self.fps);
                for output in encoder
                    .encode(planes, pts_us, duration, force_key)
                    .map_err(|error| error.to_string())?
                {
                    add(packetizer.packetize(&output.data, timestamp));
                }
            }
            Encoder::H264(encoder, packetizer) => {
                for output in encoder
                    .encode(planes, pts_us, force_key)
                    .map_err(|error| error.to_string())?
                {
                    add(packetizer.packetize(&output.data, timestamp));
                }
            }
        }
        Ok(packets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codecs_parse_by_name() {
        for name in ["vp8", "h264-superfast", "h264-ultrafast"] {
            assert_eq!(Codec::parse(name).unwrap().name(), name);
        }
        assert_eq!(
            Codec::parse("h264"),
            Some(Codec::H264(H264Preset::Superfast))
        );
        assert_eq!(Codec::parse("av1"), None);
    }

    #[test]
    fn both_codecs_packetize_frames() {
        let frame = I420Frame {
            width: 64,
            height: 48,
            y: (0..64 * 48).map(|value| value as u8).collect(),
            u: vec![128; 32 * 24],
            v: vec![128; 32 * 24],
        };
        for codec in [Codec::Vp8, Codec::H264(H264Preset::Ultrafast)] {
            let settings = EngineSettings {
                codec,
                fps: 30,
                kbps: 500,
                threads: 1,
            };
            let mut engine = Engine::new(settings, 64, 48, 1, 0).unwrap();
            // Every frame comes out at once: a still screen sends no next one.
            let packets = engine.encode(&frame, 0, true).unwrap();
            assert!(!packets.is_empty(), "{codec:?}");
            assert!(
                !engine.encode(&frame, 33_333, false).unwrap().is_empty(),
                "{codec:?}"
            );
            let payload_type = match codec {
                Codec::Vp8 => VP8_PAYLOAD_TYPE,
                Codec::H264(_) => H264_PAYLOAD_TYPE,
            };
            assert!(
                packets
                    .iter()
                    .all(|packet| packet[1] & 0x7f == payload_type)
            );
            assert!(packets.last().unwrap()[1] & 0x80 != 0);
        }
    }

    /// A raised bitrate reaches the running encoder: the same changing
    /// frames make more bytes, and an out-of-range one is refused.
    #[test]
    fn the_bitrate_changes_while_encoding() {
        let mut frame = I420Frame {
            width: 160,
            height: 96,
            y: vec![0; 160 * 96],
            u: vec![128; 80 * 48],
            v: vec![128; 80 * 48],
        };
        for codec in [Codec::Vp8, Codec::H264(H264Preset::Ultrafast)] {
            let settings = EngineSettings {
                codec,
                fps: 30,
                kbps: 100,
                threads: 1,
            };
            let mut engine = Engine::new(settings, 160, 96, 1, 0).unwrap();
            let mut seed = 7_u32;
            let mut bytes = |engine: &mut Engine, frames: i64, start: i64| -> usize {
                let mut total = 0;
                for index in start..start + frames {
                    for value in frame.y.iter_mut() {
                        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                        *value = (seed >> 16) as u8;
                    }
                    let packets = engine.encode(&frame, index * 33_333, index == 0).unwrap();
                    total += packets.iter().map(Vec::len).sum::<usize>();
                }
                total
            };
            let low = bytes(&mut engine, 30, 0);
            engine.set_bitrate(4000).unwrap();
            let high = bytes(&mut engine, 30, 30);
            assert!(high > low * 2, "{codec:?}: {low} then {high} bytes");
            assert!(engine.set_bitrate(10).is_err(), "{codec:?}");
        }
    }
}
