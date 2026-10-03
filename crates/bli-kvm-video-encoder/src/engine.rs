//! One WebRTC video codec with its RTP packetizer: VP8 (libvpx) or H.264,
//! chosen per encoder instance. H.264 comes from the board's video engine
//! where there is one that encodes, and from x264 otherwise
//! (`docs/design/h616-ve-encoder-driver-v1.md`); the board's choice moves a
//! running stream from the one to the other
//! (`docs/design/h264-engine-choice-and-quality-monitor-v1.md`).

use bli_kvm_ve_encoder::{
    device::Device,
    encoder::{Config as HardwareConfig, Counters, Error as HardwareError, HardwareEncoder, Sent},
    nv12,
};

use crate::{
    H264Packetizer, Vp8Packetizer,
    h264::{H264Config, H264Encoder, H264Preset},
    rtp::RtpError,
    vp8::{EncodedFrame, I420, Vp8Config, Vp8Encoder},
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

/// The engine's encoder, whatever device is behind it: the driver's, or a
/// stand-in in a test.
pub trait Hardware: Send {
    fn encode(
        &mut self,
        nv12: &[u8],
        force_key: bool,
        out: &mut Vec<u8>,
    ) -> Result<Sent, HardwareError>;
    fn set_bitrate(&mut self, kbps: u32);
    fn counters(&self) -> Counters;
}

impl<D: Device + Send> Hardware for HardwareEncoder<D> {
    fn encode(
        &mut self,
        nv12: &[u8],
        force_key: bool,
        out: &mut Vec<u8>,
    ) -> Result<Sent, HardwareError> {
        HardwareEncoder::encode(self, nv12, force_key, out)
    }

    fn set_bitrate(&mut self, kbps: u32) {
        HardwareEncoder::set_bitrate(self, kbps);
    }

    fn counters(&self) -> Counters {
        HardwareEncoder::counters(self)
    }
}

/// Opens the engine's encoder for a stream: the device, configured, with a
/// picture encoded to try it. An error says why there is none.
pub type OpenHardware = dyn Fn(HardwareConfig) -> Result<Box<dyn Hardware>, String> + Send + Sync;

/// After this many frames in a row that the engine failed on it is given
/// up for x264, until the engine is chosen again.
pub const HARDWARE_FAILURES: u32 = 3;

struct HardwareStream {
    encoder: Box<dyn Hardware>,
    /// The picture as the driver takes it, and the frame it makes.
    picture: Vec<u8>,
    frame: Vec<u8>,
    failures: u32,
}

enum Encoder {
    Vp8(Vp8Encoder, Vp8Packetizer),
    H264(H264Encoder, H264Packetizer),
    Hardware(Box<HardwareStream>, H264Packetizer),
    /// Between the engine and x264, for the moment the one is given up.
    None,
}

/// Encodes frames of one size and packetizes them.
pub struct Engine {
    encoder: Encoder,
    /// What an encoder for the stream is opened with; the bitrate is the
    /// last one set.
    settings: EngineSettings,
    width: usize,
    height: usize,
    /// Why the engine could not be had when it was last tried, or why it
    /// was given up.
    refusal: Option<String>,
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
        Self::with_hardware(settings, width, height, ssrc, sequence, None)
    }

    /// As `new`; for H.264 the board's engine is tried first with `open`,
    /// and x264 encodes when it cannot be opened, with a line that says why.
    pub fn with_hardware(
        settings: EngineSettings,
        width: usize,
        height: usize,
        ssrc: u32,
        sequence: u16,
        open: Option<&OpenHardware>,
    ) -> Result<Self, String> {
        let mut engine = Self {
            encoder: Encoder::None,
            settings,
            width,
            height,
            refusal: None,
        };
        engine.encoder = match settings.codec {
            Codec::Vp8 => Encoder::Vp8(
                Vp8Encoder::new(Vp8Config {
                    width: width as u32,
                    height: height as u32,
                    fps: settings.fps,
                    kbps: settings.kbps,
                    threads: settings.threads,
                    keyframe_interval: settings.fps * 2,
                })
                .map_err(|error| error.to_string())?,
                Vp8Packetizer::new(VP8_PAYLOAD_TYPE, ssrc, sequence, MTU)
                    .map_err(|error| error.to_string())?,
            ),
            Codec::H264(preset) => {
                let packetizer = H264Packetizer::new(H264_PAYLOAD_TYPE, ssrc, sequence, MTU)
                    .map_err(|error| error.to_string())?;
                match open.map(|open| engine.open_hardware(open)) {
                    Some(Ok(stream)) => Encoder::Hardware(stream, packetizer),
                    refused => {
                        if let Some(Err(reason)) = refused {
                            engine.refuse(reason);
                        }
                        Encoder::H264(
                            H264Encoder::new(engine.x264_config(preset))
                                .map_err(|error| error.to_string())?,
                            packetizer,
                        )
                    }
                }
            }
        };
        Ok(engine)
    }

    /// x264's settings for the stream.
    fn x264_config(&self, preset: H264Preset) -> H264Config {
        H264Config {
            width: self.width as u32,
            height: self.height as u32,
            fps: self.settings.fps,
            kbps: self.settings.kbps,
            threads: self.settings.threads,
            // Janus RTP input cannot request key frames; one every two
            // seconds lets joining viewers converge quickly.
            keyframe_interval: self.settings.fps * 2,
            preset,
        }
    }

    /// Opens the engine's encoder for the stream's size and bitrate.
    fn open_hardware(&self, open: &OpenHardware) -> Result<Box<HardwareStream>, String> {
        let (width, height) = (self.width as u32, self.height as u32);
        let encoder = open(HardwareConfig {
            width,
            height,
            fps: self.settings.fps,
            kbps: self.settings.kbps,
        })?;
        eprintln!("bli-kvm-video-encoder: H.264 by the video engine, {width}x{height}");
        Ok(Box::new(HardwareStream {
            encoder,
            picture: Vec::new(),
            frame: Vec::new(),
            failures: 0,
        }))
    }

    fn refuse(&mut self, reason: String) {
        eprintln!(
            "bli-kvm-video-encoder: the video engine does not encode ({reason}); H.264 by x264"
        );
        self.refusal = Some(reason);
    }

    /// Why the engine could not be had when it was last tried for this
    /// stream, or why it was given up; `None` where it was had or never
    /// tried.
    pub fn hardware_refusal(&self) -> Option<&str> {
        self.refusal.as_deref()
    }

    /// The board chose x264: where the engine encodes, x264 takes the
    /// stream over where it is, behind the same packetizer, beginning with
    /// a key frame. That is no failure of the engine. An error is x264's,
    /// which could not be opened.
    pub fn use_software(&mut self) -> Result<(), String> {
        if matches!(self.encoder, Encoder::Hardware(..)) {
            eprintln!("bli-kvm-video-encoder: x264 is chosen; H.264 by x264 from here on");
            self.x264_takes_over()?;
        }
        Ok(())
    }

    /// The board chose the engine: where x264 encodes H.264, the engine is
    /// opened for the stream and takes it over behind the same packetizer;
    /// its first frame is a key frame. Where it cannot be had x264 goes on,
    /// and the error says why.
    pub fn use_hardware(&mut self, open: &OpenHardware) -> Result<(), String> {
        if !matches!(self.encoder, Encoder::H264(..)) {
            return Ok(());
        }
        // x264 goes only once the engine is had.
        let stream = self.open_hardware(open).inspect_err(|reason| {
            self.refuse(reason.clone());
        })?;
        let Encoder::H264(x264, packetizer) = std::mem::replace(&mut self.encoder, Encoder::None)
        else {
            unreachable!("x264 encodes");
        };
        drop(x264);
        self.encoder = Encoder::Hardware(stream, packetizer);
        self.refusal = None;
        Ok(())
    }

    /// x264 takes the stream over from the engine where it is, with the
    /// same packetizer.
    fn x264_takes_over(&mut self) -> Result<(), String> {
        let Codec::H264(preset) = self.settings.codec else {
            unreachable!("only H.264 has the engine");
        };
        let Encoder::Hardware(stream, packetizer) =
            std::mem::replace(&mut self.encoder, Encoder::None)
        else {
            unreachable!("the engine encodes");
        };
        // The device is closed before x264 takes its threads.
        drop(stream);
        let encoder =
            H264Encoder::new(self.x264_config(preset)).map_err(|error| error.to_string())?;
        self.encoder = Encoder::H264(encoder, packetizer);
        Ok(())
    }

    /// Changes the target bitrate of the running encoder; the stream goes
    /// on, and an encoder that takes it over later starts from it.
    pub fn set_bitrate(&mut self, kbps: u32) -> Result<(), String> {
        match &mut self.encoder {
            Encoder::Vp8(encoder, _) => {
                encoder
                    .set_bitrate(kbps)
                    .map_err(|error| error.to_string())?;
            }
            Encoder::H264(encoder, _) => {
                encoder
                    .set_bitrate(kbps)
                    .map_err(|error| error.to_string())?;
            }
            Encoder::Hardware(stream, _) => {
                if !crate::demand::KBPS_RANGE.contains(&kbps) {
                    return Err("the bitrate is out of range".into());
                }
                stream.encoder.set_bitrate(kbps);
            }
            Encoder::None => return Err("no encoder".into()),
        }
        self.settings.kbps = kbps;
        Ok(())
    }

    /// The sequence number the next RTP packet carries.
    pub fn next_sequence(&self) -> u16 {
        match &self.encoder {
            Encoder::Vp8(_, packetizer) => packetizer.next_sequence(),
            Encoder::H264(_, packetizer) | Encoder::Hardware(_, packetizer) => {
                packetizer.next_sequence()
            }
            Encoder::None => 0,
        }
    }

    /// What encodes: `vp8`, `x264` or `hardware`.
    pub fn encoder_name(&self) -> &'static str {
        match &self.encoder {
            Encoder::Vp8(..) => "vp8",
            Encoder::H264(..) | Encoder::None => "x264",
            Encoder::Hardware(..) => "hardware",
        }
    }

    /// What the engine's encoder has done, where it encodes.
    pub fn hardware_counters(&self) -> Option<Counters> {
        match &self.encoder {
            Encoder::Hardware(stream, _) => Some(stream.encoder.counters()),
            _ => None,
        }
    }

    /// The engine is given up: x264 takes the stream over where it is,
    /// with the same packetizer.
    fn give_up_hardware(&mut self) -> Result<(), String> {
        eprintln!(
            "bli-kvm-video-encoder: the video engine failed on {HARDWARE_FAILURES} frames in a row; H.264 by x264 from here on"
        );
        self.refusal = Some(format!(
            "the engine failed on {HARDWARE_FAILURES} frames in a row"
        ));
        self.x264_takes_over()
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
        Ok(self.encode_frames(frame, pts_us, force_key)?.0)
    }

    /// Encodes one frame taken at `pts_us`: its RTP packets, and the encoded
    /// frames themselves (for H.264 one access unit in Annex B form, or none).
    pub fn encode_frames(
        &mut self,
        frame: &I420Frame,
        pts_us: i64,
        force_key: bool,
    ) -> Result<(Vec<Vec<u8>>, Vec<EncodedFrame>), String> {
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
        let frames = match &mut self.encoder {
            Encoder::Vp8(encoder, packetizer) => {
                let duration = 1_000_000 / i64::from(self.settings.fps);
                let frames = encoder
                    .encode(planes, pts_us, duration, force_key)
                    .map_err(|error| error.to_string())?;
                for output in &frames {
                    add(packetizer.packetize(&output.data, timestamp));
                }
                frames
            }
            Encoder::H264(encoder, packetizer) => {
                let frames = encoder
                    .encode(planes, pts_us, force_key)
                    .map_err(|error| error.to_string())?;
                for output in &frames {
                    add(packetizer.packetize(&output.data, timestamp));
                }
                frames
            }
            Encoder::Hardware(stream, packetizer) => {
                nv12::from_i420(
                    &frame.y,
                    &frame.u,
                    &frame.v,
                    frame.width,
                    frame.height,
                    &mut stream.picture,
                );
                match stream
                    .encoder
                    .encode(&stream.picture, force_key, &mut stream.frame)
                {
                    Ok(sent) => {
                        stream.failures = 0;
                        add(packetizer.packetize(&stream.frame, timestamp));
                        vec![EncodedFrame {
                            data: stream.frame.clone(),
                            key_frame: sent.key,
                        }]
                    }
                    Err(error) => {
                        stream.failures += 1;
                        eprintln!("bli-kvm-video-encoder: the video engine: {error}");
                        if stream.failures < HARDWARE_FAILURES {
                            // The frame is lost; the next one is a key frame.
                            return Ok((Vec::new(), Vec::new()));
                        }
                        self.give_up_hardware()?;
                        return self.encode_frames(frame, pts_us, true);
                    }
                }
            }
            Encoder::None => return Err("no encoder".into()),
        };
        Ok((packets, frames))
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

    fn standin(fail_from: Option<usize>) -> Box<OpenHardware> {
        use bli_kvm_ve_encoder::standin::StandinDevice;
        Box::new(move |config| {
            let mut device = StandinDevice::new();
            device.fail_from = fail_from;
            HardwareEncoder::new(device, config)
                .map(|encoder| Box::new(encoder) as Box<dyn Hardware>)
                .map_err(|error| error.to_string())
        })
    }

    fn grey(width: usize, height: usize, shade: u8) -> I420Frame {
        I420Frame {
            width,
            height,
            y: vec![shade; width * height],
            u: vec![128; width * height / 4],
            v: vec![128; width * height / 4],
        }
    }

    const H264: EngineSettings = EngineSettings {
        codec: Codec::H264(H264Preset::Ultrafast),
        fps: 30,
        kbps: 2500,
        threads: 1,
    };

    /// The header bytes of an access unit's units.
    fn unit_types(frame: &[u8]) -> Vec<u8> {
        frame
            .windows(5)
            .filter(|window| window[..4] == [0, 0, 0, 1])
            .map(|window| window[4] & 0x1f)
            .collect()
    }

    #[test]
    fn h264_comes_from_the_engine_where_there_is_one() {
        let open = standin(None);
        let mut engine = Engine::with_hardware(H264, 320, 240, 7, 100, Some(&*open)).unwrap();
        assert_eq!(engine.encoder_name(), "hardware");
        let (packets, frames) = engine.encode_frames(&grey(320, 240, 60), 0, true).unwrap();
        assert_eq!(frames.len(), 1);
        assert!(frames[0].key_frame);
        // SPS, PPS and the key frame's slice, as from x264.
        assert_eq!(unit_types(&frames[0].data), [7, 8, 5]);
        assert!(!packets.is_empty());
        assert!(
            packets
                .iter()
                .all(|packet| packet[1] & 0x7f == H264_PAYLOAD_TYPE)
        );
        assert!(packets.last().unwrap()[1] & 0x80 != 0);
        let (packets, frames) = engine
            .encode_frames(&grey(320, 240, 61), 33_333, false)
            .unwrap();
        assert!(!frames[0].key_frame && !packets.is_empty());
        assert_eq!(unit_types(&frames[0].data), [1]);
        // The bitrate reaches it, and one out of range is refused.
        engine.set_bitrate(8000).unwrap();
        assert!(engine.set_bitrate(10).is_err());
        let counters = engine.hardware_counters().unwrap();
        assert_eq!((counters.frames, counters.key_frames), (2, 1));
        // VP8 is never the engine's.
        let vp8 = EngineSettings {
            codec: Codec::Vp8,
            ..H264
        };
        let engine = Engine::with_hardware(vp8, 320, 240, 7, 100, Some(&*open)).unwrap();
        assert_eq!(engine.encoder_name(), "vp8");
    }

    #[test]
    fn x264_encodes_where_the_engine_does_not() {
        // A size the engine's driver does not take, and an engine that
        // fails when it is tried.
        let open = standin(None);
        let engine = Engine::with_hardware(H264, 64, 48, 7, 100, Some(&*open)).unwrap();
        assert_eq!(engine.encoder_name(), "x264");
        assert!(engine.hardware_counters().is_none());
        let failing = standin(Some(0));
        let mut engine = Engine::with_hardware(H264, 320, 240, 7, 100, Some(&*failing)).unwrap();
        assert_eq!(engine.encoder_name(), "x264");
        let (packets, _) = engine.encode_frames(&grey(320, 240, 60), 0, true).unwrap();
        assert!(!packets.is_empty());
        // No engine is named: x264, as before.
        let engine = Engine::new(H264, 320, 240, 7, 100).unwrap();
        assert_eq!(engine.encoder_name(), "x264");
    }

    #[test]
    fn an_engine_that_fails_on_three_frames_in_a_row_is_given_up_for_x264() {
        // The try and two frames go well; from then on every pass fails.
        let open = standin(Some(3));
        let mut engine = Engine::with_hardware(H264, 320, 240, 7, 100, Some(&*open)).unwrap();
        for index in 0..2 {
            let (packets, _) = engine
                .encode_frames(
                    &grey(320, 240, 60 + index),
                    index as i64 * 33_333,
                    index == 0,
                )
                .unwrap();
            assert!(!packets.is_empty());
        }
        let before = engine.next_sequence();
        // Two frames are lost.
        for index in 2..4 {
            let (packets, frames) = engine
                .encode_frames(&grey(320, 240, 60 + index), index as i64 * 33_333, false)
                .unwrap();
            assert!(packets.is_empty() && frames.is_empty());
            assert_eq!(engine.encoder_name(), "hardware");
        }
        // The third is x264's, a key frame, in the same stream of packets.
        let (packets, frames) = engine
            .encode_frames(&grey(320, 240, 64), 4 * 33_333, false)
            .unwrap();
        assert_eq!(engine.encoder_name(), "x264");
        assert!(frames[0].key_frame);
        assert_eq!(u16::from_be_bytes([packets[0][2], packets[0][3]]), before);
        let (packets, frames) = engine
            .encode_frames(&grey(320, 240, 65), 5 * 33_333, false)
            .unwrap();
        assert!(!packets.is_empty() && !frames[0].key_frame);
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
