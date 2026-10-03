//! A stream from the engine: pictures in, access units out. The headers,
//! the QP of every frame, the choice between the engine's two searches,
//! frames encoded once more and frames left out are decided here; the
//! device encodes a picture at a QP.

use std::{io, time::Duration};

use crate::{
    device::{Device, Pass, Picture, Reference, Sizes},
    headers::{self, RawUnit, Slice, Stream},
    policy::{self, Ask, FramePolicy, Next, Quantiser, Searching, Which},
    rate::{RateControl, Search, SearchChoice},
    skip,
};

/// What a stream is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// The picture's visible size: even numbers.
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub kbps: u32,
}

/// What the encoder has done since it began.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counters {
    /// Frames that went out, key frames and frames left out among them.
    pub frames: u64,
    pub key_frames: u64,
    /// Frames in whose place a frame of skipped macroblocks went out.
    pub left_out: u64,
    /// Passes of the engine.
    pub passes: u64,
    /// Frames encoded with both searches.
    pub both_searches: u64,
    /// Frames that went out from the wide search.
    pub wide: u64,
    /// Frames encoded once more at a higher QP.
    pub higher_qp: u64,
    /// Frames the engine failed on.
    pub failures: u64,
    /// The engine's time in all passes, in microseconds.
    pub engine_micros: u64,
}

#[derive(Debug)]
pub enum Error {
    /// The picture is not of the stream's size.
    Picture,
    /// The size is not one the device takes, or its answer is not the
    /// size of the stream's pictures.
    Size,
    /// The device failed; the next frame is a key frame.
    Device(io::Error),
    /// The device made something that is no frame.
    Frame,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Picture => write!(formatter, "the picture is not of the stream's size"),
            Self::Size => write!(formatter, "the device does not take this size"),
            Self::Device(error) => write!(formatter, "the device failed: {error}"),
            Self::Frame => write!(formatter, "the device made no frame"),
        }
    }
}

impl std::error::Error for Error {}

/// One frame that went out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    pub key: bool,
    /// The frame says "nothing changed": the picture did not fit the rate.
    pub left_out: bool,
    pub qp: i32,
    /// Passes the engine made for it.
    pub passes: u32,
    /// The engine's time in them.
    pub engine: Duration,
}

pub struct HardwareEncoder<D: Device> {
    device: D,
    stream: Stream,
    sizes: Sizes,
    policy: FramePolicy,
    frame_num: u32,
    idr_pic_id: u32,
    /// What the next new picture refers to.
    reference: Reference,
    /// A key frame is due: the stream begins, or the engine failed.
    need_key: bool,
    /// The last pass over the picture, and the one that is kept beside it.
    last: Vec<u8>,
    kept: Vec<u8>,
    counters: Counters,
}

/// Begins with a start code and holds more than a header.
fn is_frame(bytes: &[u8]) -> bool {
    bytes.len() > 5 && bytes[..4] == [0, 0, 0, 1]
}

impl<D: Device> HardwareEncoder<D> {
    /// Configures the device and encodes one grey picture with it, which
    /// is thrown away: a device that cannot do that is no encoder.
    pub fn new(mut device: D, config: Config) -> Result<Self, Error> {
        if config.width % 2 != 0 || config.height % 2 != 0 || config.fps == 0 {
            return Err(Error::Size);
        }
        let stream = Stream {
            width: config.width,
            height: config.height,
            fps: config.fps,
            cabac: false,
            // This generation of the engine has no field for it.
            chroma_qp_offset: 0,
            deblocking: true,
        };
        let sizes = device
            .configure(config.width, config.height, false)
            .map_err(Error::Device)?;
        let coded = (stream.coded_width() * stream.coded_height()) as usize;
        if sizes.picture_bytes != coded * 3 / 2 {
            return Err(Error::Size);
        }
        let quantiser = Quantiser::Rate(RateControl::new(config.kbps, config.fps));
        let costly = FramePolicy::costly_bits(&quantiser, config.kbps, config.fps, 0);
        // As many passes as end within a frame's time.
        let passes = policy::passes_for(coded as u64, config.fps);
        let mut encoder = Self {
            device,
            stream,
            sizes,
            policy: FramePolicy::new(quantiser, Searching::Chosen(SearchChoice::new()), costly)
                .with_passes(passes),
            frame_num: 0,
            idr_pic_id: 0,
            reference: Reference::Last,
            need_key: true,
            last: Vec::new(),
            kept: Vec::new(),
            counters: Counters::default(),
        };
        let grey = vec![128u8; sizes.picture_bytes];
        let ask = Ask {
            qp: headers::INITIAL_QP,
            search: Some(Search::Exact),
        };
        encoder
            .pass(
                Picture::New {
                    nv12: &grey,
                    reference: Reference::Last,
                },
                true,
                ask,
            )
            .map_err(Error::Device)?;
        if !is_frame(&encoder.last) {
            return Err(Error::Frame);
        }
        encoder.counters = Counters::default();
        Ok(encoder)
    }

    pub fn config_stream(&self) -> &Stream {
        &self.stream
    }

    pub fn counters(&self) -> Counters {
        self.counters
    }

    pub fn device(&self) -> &D {
        &self.device
    }

    /// The bytes of a picture: NV12 of whole macroblocks.
    pub fn picture_bytes(&self) -> usize {
        self.sizes.picture_bytes
    }

    /// Another rate for the stream as it runs.
    pub fn set_bitrate(&mut self, kbps: u32) {
        self.policy.set_rate(kbps, self.stream.fps);
    }

    /// One pass into `self.last`.
    fn pass(&mut self, picture: Picture<'_>, key: bool, ask: Ask) -> io::Result<Duration> {
        let slice = Slice {
            key,
            frame_num: if key { 0 } else { self.frame_num },
            idr_pic_id: self.idr_pic_id,
            qp: ask.qp,
            cabac_init_idc: 0,
        };
        let mut ops = Vec::new();
        if key {
            ops.extend(headers::sps(&self.stream));
            ops.extend(headers::pps(&self.stream));
        }
        ops.extend(headers::slice_header(&self.stream, &slice));
        let units: Vec<RawUnit> = headers::units(&ops);
        let pass = Pass {
            key,
            qp: ask.qp as u32,
            search: ask.search.unwrap_or(Search::Exact),
            picture,
        };
        let made = self.device.encode(&pass, &units, &mut self.last)?;
        self.counters.passes += 1;
        self.counters.engine_micros += made.time.as_micros() as u64;
        Ok(made.time)
    }

    /// Encodes `nv12` as the stream's next frame and puts the access unit
    /// into `out`: a key frame with its parameter sets when one is asked
    /// for or due. After an error nothing went out and `out` is empty; the
    /// next frame is a key frame.
    pub fn encode(
        &mut self,
        nv12: &[u8],
        force_key: bool,
        out: &mut Vec<u8>,
    ) -> Result<Sent, Error> {
        out.clear();
        if nv12.len() != self.sizes.picture_bytes {
            return Err(Error::Picture);
        }
        let key = force_key || self.need_key;
        let tried = self.policy.choice().map_or(0, |choice| choice.tried);
        let outcome = self.passes(nv12, key);
        if self
            .policy
            .choice()
            .is_some_and(|choice| choice.tried != tried)
        {
            self.counters.both_searches += 1;
        }
        let (which, passes, engine, higher) = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                // What the engine left is no frame, and the device has
                // forgotten the stream: it begins again.
                self.policy.failed();
                self.need_key = true;
                self.reference = Reference::Last;
                self.counters.failures += 1;
                return Err(error);
            }
        };
        if higher {
            self.counters.higher_qp += 1;
        }
        self.counters.frames += 1;
        let Some(which) = which else {
            // Left out: a frame of skipped macroblocks in its place, and
            // the stream stays where it was.
            *out = skip::unchanged_frame(&self.stream, self.frame_num);
            self.frame_num += 1;
            self.policy.left_out(out.len() as u64 * 8);
            self.reference = Reference::Stay;
            self.counters.left_out += 1;
            return Ok(Sent {
                key: false,
                left_out: true,
                qp: headers::INITIAL_QP,
                passes,
                engine,
            });
        };
        let (frame, reference) = match which {
            Which::Last => (&self.last, Reference::Last),
            Which::Earlier => (&self.kept, Reference::Earlier),
        };
        out.extend_from_slice(frame);
        self.reference = reference;
        let qp = self.policy.sent();
        self.need_key = false;
        self.frame_num = if key { 1 } else { self.frame_num + 1 };
        if key {
            self.idr_pic_id ^= 1;
            self.counters.key_frames += 1;
        } else if self.policy.sent_search() == Some(Search::Wide) {
            self.counters.wide += 1;
        }
        Ok(Sent {
            key,
            left_out: false,
            qp,
            passes,
            engine,
        })
    }

    /// The passes of one frame: which of them is the frame (none: it is
    /// left out), how many were made, the engine's time, and whether one
    /// was at a higher QP.
    #[allow(clippy::type_complexity)]
    fn passes(
        &mut self,
        nv12: &[u8],
        key: bool,
    ) -> Result<(Option<Which>, u32, Duration, bool), Error> {
        let first = self.policy.begin(key);
        let picture = Picture::New {
            nv12,
            reference: self.reference,
        };
        let mut engine = self.pass(picture, key, first).map_err(Error::Device)?;
        let (mut passes, mut higher) = (1, false);
        loop {
            if !is_frame(&self.last) {
                return Err(Error::Frame);
            }
            match self.policy.made(self.last.len() as u64 * 8, key) {
                Next::Again { ask, better } => {
                    // The driver keeps the last pass when it makes another;
                    // so does this: the better of the two so far is kept.
                    if better == Which::Last {
                        std::mem::swap(&mut self.last, &mut self.kept);
                    }
                    higher |= ask.qp != first.qp;
                    engine += self.pass(Picture::Again, key, ask).map_err(Error::Device)?;
                    passes += 1;
                }
                Next::Send(which) => return Ok((Some(which), passes, engine, higher)),
                Next::LeaveOut => return Ok((None, passes, engine, higher)),
            }
        }
    }
}

#[cfg(all(test, feature = "standin"))]
mod tests {
    use super::*;
    use crate::{
        abi,
        bits::{BitSource, without_prevention},
        nv12,
        standin::StandinDevice,
    };

    const CONFIG: Config = Config {
        width: 320,
        height: 240,
        fps: 30,
        kbps: 2500,
    };

    fn picture(shade: u8) -> Vec<u8> {
        vec![shade; nv12::picture_bytes(320, 240)]
    }

    /// The units of an access unit: each one's header byte and payload.
    fn units(bytes: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut starts = Vec::new();
        let mut at = 0;
        while at + 4 <= bytes.len() {
            if bytes[at..at + 4] == [0, 0, 0, 1] {
                starts.push(at);
                at += 4;
            } else {
                at += 1;
            }
        }
        starts
            .iter()
            .enumerate()
            .map(|(index, start)| {
                let end = starts.get(index + 1).copied().unwrap_or(bytes.len());
                (bytes[start + 4], bytes[start + 5..end].to_vec())
            })
            .collect()
    }

    /// `frame_num` of a slice that is not a key frame's.
    fn frame_num(payload: &[u8]) -> u32 {
        let payload = without_prevention(payload);
        let mut source = BitSource::new(&payload);
        source.ue(); // first_mb_in_slice
        source.ue(); // slice_type
        source.ue(); // pic_parameter_set_id
        source.bits(headers::FRAME_NUM_BITS).unwrap()
    }

    /// A device whose frames cost what a model says: `cost` bits at QP 30
    /// for the exact search, halving with six steps of QP; a key frame
    /// eight times that. The stand-in's own frames are smaller than any of
    /// it at this size but a key frame's, which carries its samples.
    fn modelled(cost: impl Fn(&Pass<'_>) -> f64 + Send + 'static) -> StandinDevice {
        let mut device = StandinDevice::new();
        device.size = Some(Box::new(move |pass, _| {
            let bits = cost(pass) * 2f64.powf((30.0 - pass.qp as f64) / 6.0);
            (if pass.key { bits * 8.0 } else { bits } / 8.0) as usize
        }));
        device
    }

    #[test]
    fn a_stream_begins_with_a_key_frame_and_its_parameter_sets() {
        let mut encoder = HardwareEncoder::new(StandinDevice::new(), CONFIG).unwrap();
        assert_eq!(encoder.picture_bytes(), 320 * 240 * 3 / 2);
        // The picture that was encoded to try the device does not count.
        assert_eq!(encoder.counters(), Counters::default());
        let mut out = Vec::new();
        let sent = encoder.encode(&picture(90), false, &mut out).unwrap();
        assert!(sent.key && !sent.left_out && sent.passes == 1);
        let key = units(&out);
        assert_eq!(
            key.iter().map(|unit| unit.0).collect::<Vec<_>>(),
            [0x67, 0x68, 0x65]
        );
        // Constrained baseline, the level of 320x240 at 30.
        assert_eq!(key[0].1[..3], [66, 0xc0, 30]);

        for expected in 1..=3 {
            let sent = encoder.encode(&picture(91), false, &mut out).unwrap();
            assert!(!sent.key);
            let frame = units(&out);
            assert_eq!(frame.len(), 1);
            assert_eq!(frame[0].0, 0x41);
            assert_eq!(frame_num(&frame[0].1), expected);
        }
        // A key frame when it is asked for, with the other of the two
        // identifiers, and the frames count on from it.
        let sent = encoder.encode(&picture(92), true, &mut out).unwrap();
        assert!(sent.key);
        assert_eq!(units(&out)[2].0, 0x65);
        encoder.encode(&picture(93), false, &mut out).unwrap();
        assert_eq!(frame_num(&units(&out)[0].1), 1);
        let counters = encoder.counters();
        assert_eq!((counters.frames, counters.key_frames), (6, 2));
        assert_eq!(
            (counters.passes, counters.left_out, counters.failures),
            (6, 0, 0)
        );
        // What the device was asked: the try, then the stream. Nothing
        // names a reference: every picture refers to the last pass.
        let flags: Vec<u32> = encoder.device().passes.iter().map(|pass| pass.0).collect();
        assert_eq!(flags, [abi::KEY, abi::KEY, 0, 0, 0, abi::KEY, 0]);
    }

    #[test]
    fn a_picture_of_another_size_is_refused_and_so_is_a_size_the_device_does_not_take() {
        let mut encoder = HardwareEncoder::new(StandinDevice::new(), CONFIG).unwrap();
        let mut out = vec![1, 2, 3];
        assert!(matches!(
            encoder.encode(&picture(1)[1..], false, &mut out),
            Err(Error::Picture)
        ));
        assert!(out.is_empty());
        for (width, height) in [(32, 32), (1922, 1080), (321, 240), (4096, 2160)] {
            let config = Config {
                width,
                height,
                ..CONFIG
            };
            assert!(HardwareEncoder::new(StandinDevice::new(), config).is_err());
        }
    }

    #[test]
    fn a_costly_frame_is_tried_with_both_searches_and_the_better_pass_is_the_frame() {
        // The wide search makes a tenth of the exact one's bits; a key
        // frame fits the bucket.
        let device = modelled(|pass| match (pass.key, pass.search) {
            (true, _) => 20_000.0,
            (false, Search::Wide) => 40_000.0,
            (false, Search::Exact) => 400_000.0,
        });
        let mut encoder = HardwareEncoder::new(device, CONFIG).unwrap();
        let mut out = Vec::new();
        encoder.encode(&picture(10), false, &mut out).unwrap();
        let sent = encoder.encode(&picture(11), false, &mut out).unwrap();
        assert_eq!(sent.passes, 2);
        assert_eq!(out.len(), 40_000 / 8);
        // The next frame begins with the wide search and is not tried twice.
        let sent = encoder.encode(&picture(12), false, &mut out).unwrap();
        assert_eq!(sent.passes, 1);
        let passes = &encoder.device().passes;
        let flags: Vec<u32> = passes.iter().map(|pass| pass.0).collect();
        assert_eq!(
            flags[2..],
            [0, abi::AGAIN | abi::WIDE, abi::WIDE],
            "{flags:?}"
        );
        // The picture after the two passes refers to the second of them,
        // which read what the first read.
        assert_eq!(passes[2].2, passes[3].2);
        assert_ne!(passes[4].2, passes[3].2);
        let counters = encoder.counters();
        assert_eq!((counters.both_searches, counters.wide), (1, 2));
    }

    #[test]
    fn when_the_other_search_is_no_better_the_first_pass_is_the_frame() {
        // The wide search makes a fifth more.
        let device = modelled(|pass| match (pass.key, pass.search) {
            (true, _) => 20_000.0,
            (false, Search::Wide) => 480_000.0,
            (false, Search::Exact) => 400_000.0,
        });
        let mut encoder = HardwareEncoder::new(device, CONFIG).unwrap();
        let mut out = Vec::new();
        encoder.encode(&picture(10), false, &mut out).unwrap();
        let sent = encoder.encode(&picture(11), false, &mut out).unwrap();
        assert_eq!(sent.passes, 2);
        assert_eq!(out.len(), 400_000 / 8);
        encoder.encode(&picture(12), false, &mut out).unwrap();
        let flags: Vec<u32> = encoder.device().passes.iter().map(|pass| pass.0).collect();
        // The next picture says that it refers to the pass before the last.
        assert_eq!(
            flags[2..5],
            [0, abi::AGAIN | abi::WIDE, abi::EARLIER],
            "{flags:?}"
        );
        assert_eq!(encoder.counters().wide, 0);
    }

    #[test]
    fn a_frame_that_fits_at_no_qp_is_left_out_and_the_stream_stays_where_it_was() {
        // A key frame of 1.6 Mbit, then frames of 8 Mbit at QP 30: more
        // than the bucket of 2.5 Mbit has room for at any QP.
        let device = modelled(|pass| if pass.key { 200_000.0 } else { 100_000_000.0 });
        let mut encoder = HardwareEncoder::new(device, CONFIG).unwrap();
        let mut out = Vec::new();
        encoder.encode(&picture(10), false, &mut out).unwrap();
        let sent = encoder.encode(&picture(11), false, &mut out).unwrap();
        assert!(sent.left_out && !sent.key);
        // A frame of skipped macroblocks, with the number the frame has.
        let frame = units(&out);
        assert_eq!((frame.len(), frame[0].0), (1, 0x41));
        assert!(out.len() < 16);
        assert_eq!(frame_num(&frame[0].1), 1);
        let sent = encoder.encode(&picture(12), false, &mut out).unwrap();
        assert!(sent.left_out);
        assert_eq!(frame_num(&units(&out)[0].1), 2);
        // Every picture after a frame that was left out says that the
        // stream stayed, and reads what the frame before it read.
        let passes = &encoder.device().passes;
        let key = passes[1].2;
        let news: Vec<(u32, usize)> = passes[2..]
            .iter()
            .filter(|pass| pass.0 & abi::AGAIN == 0)
            .map(|pass| (pass.0 & (abi::STAY | abi::EARLIER), pass.2))
            .collect();
        assert_eq!(news, [(0, key), (abi::STAY, key)]);
        assert_eq!(encoder.counters().left_out, 2);
        assert_eq!(encoder.counters().frames, 3);
    }

    #[test]
    fn after_a_failure_the_stream_begins_again_with_a_key_frame() {
        let mut device = StandinDevice::new();
        // The try and three passes go well; the fourth does not.
        device.fail_in = Some(4);
        let mut encoder = HardwareEncoder::new(device, CONFIG).unwrap();
        let mut out = Vec::new();
        for shade in 0..3 {
            encoder.encode(&picture(shade), false, &mut out).unwrap();
        }
        assert!(matches!(
            encoder.encode(&picture(3), false, &mut out),
            Err(Error::Device(_))
        ));
        assert!(out.is_empty());
        assert_eq!(encoder.counters().failures, 1);
        let sent = encoder.encode(&picture(4), false, &mut out).unwrap();
        assert!(sent.key);
        assert_eq!(units(&out)[0].0, 0x67);
        encoder.encode(&picture(5), false, &mut out).unwrap();
        assert_eq!(frame_num(&units(&out)[0].1), 1);
        // A device that fails when it is tried is no encoder.
        let mut device = StandinDevice::new();
        device.fail_in = Some(0);
        assert!(matches!(
            HardwareEncoder::new(device, CONFIG),
            Err(Error::Device(_))
        ));
    }

    #[test]
    fn the_rate_is_held_and_follows_when_it_is_changed() {
        // 150 kbit a frame at QP 30: far more than 2.5 Mbit/s carries.
        let device = modelled(|_| 150_000.0);
        let mut encoder = HardwareEncoder::new(device, CONFIG).unwrap();
        let mut out = Vec::new();
        let mut bits = |encoder: &mut HardwareEncoder<StandinDevice>, frames: u32| -> f64 {
            let mut total = 0;
            for frame in 0..frames {
                encoder
                    .encode(&picture(frame as u8), frame % 60 == 0, &mut out)
                    .unwrap();
                total += out.len() * 8;
            }
            total as f64
        };
        bits(&mut encoder, 90);
        let ten_seconds = bits(&mut encoder, 300);
        assert!(
            (0.9..1.1).contains(&(ten_seconds / 25_000_000.0)),
            "{ten_seconds}"
        );
        encoder.set_bitrate(8000);
        bits(&mut encoder, 90);
        let ten_seconds = bits(&mut encoder, 300);
        assert!(
            (0.9..1.1).contains(&(ten_seconds / 80_000_000.0)),
            "{ten_seconds}"
        );
        assert_eq!(encoder.counters().failures, 0);
    }
}
