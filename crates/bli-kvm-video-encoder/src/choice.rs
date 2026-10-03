//! The board's choice of the H.264 engine, as the encoder keeps it
//! (`docs/design/h264-engine-choice-and-quality-monitor-v1.md`): what the
//! server last chose, whether the engine can be had and why not, and when
//! an engine that failed is tried again.
//!
//! The engine is wanted until the server chooses x264. A stream that opens
//! tries it where it is wanted; a running stream moves to it or away from it
//! between two frames. An engine that failed on a stream, or that could not
//! be opened when it was chosen, is not tried again until the choice is made
//! again; the second also a new stream tries again.

use crate::{
    demand::EngineChoice,
    engine::{Engine, OpenHardware},
};

/// Opens the engine's device, asks what it is and closes it again: whether
/// the engine can be had, or why not.
pub type ProbeHardware = dyn Fn() -> Result<(), String> + Send + Sync;

/// A reason is cut to this many characters for the report.
pub const MAX_REASON_CHARS: usize = 120;

/// `reason`, cut to [`MAX_REASON_CHARS`] characters.
pub fn short_reason(reason: &str) -> String {
    reason.chars().take(MAX_REASON_CHARS).collect()
}

pub struct Choice {
    /// Where the unit names the engine's device: how to open it for a
    /// stream, and how to look at it.
    open: Option<Box<OpenHardware>>,
    probe: Option<Box<ProbeHardware>>,
    /// How many choices the server has made, and the last one.
    lines: u64,
    wanted: EngineChoice,
    /// Whether the engine can be had: `None` where no device is named.
    available: Option<bool>,
    reason: Option<String>,
    /// The engine failed on a stream after this many choices.
    given_up_at: Option<u64>,
    /// The engine could not be opened for this stream after this many
    /// choices.
    refused_at: Option<u64>,
}

impl Choice {
    /// Looks at the device at once, where one is named.
    pub fn new(open: Option<Box<OpenHardware>>, probe: Option<Box<ProbeHardware>>) -> Self {
        let mut choice = Self {
            open,
            probe,
            lines: 0,
            wanted: EngineChoice::Hardware,
            available: None,
            reason: None,
            given_up_at: None,
            refused_at: None,
        };
        choice.look();
        choice
    }

    fn look(&mut self) {
        if let Some(probe) = &self.probe {
            self.found(probe());
        }
    }

    fn found(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.available = Some(true);
                self.reason = None;
            }
            Err(reason) => {
                self.available = Some(false);
                self.reason = Some(short_reason(&reason));
            }
        }
    }

    /// Takes the server's choices: `lines` made so far, the last `chosen`.
    /// Where nothing else is about to try the engine, a look at the device
    /// says whether it can be had, so that the choice can be made again.
    pub fn take(&mut self, lines: u64, chosen: Option<EngineChoice>, engine: Option<&Engine>) {
        if lines == self.lines {
            return;
        }
        self.lines = lines;
        self.wanted = chosen.unwrap_or(EngineChoice::Hardware);
        let encodes = engine.is_some_and(|engine| engine.encoder_name() == "hardware");
        let about_to_try = engine.is_some() && self.wanted == EngineChoice::Hardware;
        if !encodes && !about_to_try {
            self.look();
        }
    }

    /// What a new stream opens the engine with: where it is wanted and has
    /// not failed since the choice was last made.
    pub fn opener(&self) -> Option<&OpenHardware> {
        (self.wanted == EngineChoice::Hardware && self.given_up_at != Some(self.lines))
            .then_some(self.open.as_deref())
            .flatten()
    }

    /// A new stream opened, with the engine tried (`tried`) or not.
    pub fn opened(&mut self, engine: &Engine, tried: bool) {
        self.refused_at = None;
        if tried {
            let refusal = engine.hardware_refusal().map(str::to_string);
            if refusal.is_some() {
                // x264 goes on, and the engine is not tried at every frame.
                self.refused_at = Some(self.lines);
            }
            self.found(refusal.map_or(Ok(()), Err));
        }
    }

    /// Moves the running stream to the engine chosen, where it can. An
    /// error is x264's, which could not be opened.
    pub fn apply(&mut self, engine: &mut Engine) -> Result<(), String> {
        match self.wanted {
            EngineChoice::Software => engine.use_software(),
            EngineChoice::Hardware => {
                let Some(open) = self.open.as_deref() else {
                    return Ok(());
                };
                if engine.encoder_name() != "x264"
                    || self.given_up_at == Some(self.lines)
                    || self.refused_at == Some(self.lines)
                {
                    return Ok(());
                }
                let result = engine.use_hardware(open);
                if result.is_err() {
                    self.refused_at = Some(self.lines);
                }
                self.found(result);
                Ok(())
            }
        }
    }

    /// The engine failed on the stream and x264 took it over.
    pub fn given_up(&mut self, engine: &Engine) {
        self.given_up_at = Some(self.lines);
        self.found(Err(engine
            .hardware_refusal()
            .unwrap_or("the engine failed")
            .to_string()));
    }

    pub fn wanted(&self) -> EngineChoice {
        self.wanted
    }

    pub fn available(&self) -> Option<bool> {
        self.available
    }

    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    };

    use bli_kvm_ve_encoder::{encoder::HardwareEncoder, standin::StandinDevice};

    use super::*;
    use crate::{
        engine::{Codec, EngineSettings, Hardware},
        frames::Numbering,
        h264::H264Preset,
        yuv::I420Frame,
    };

    const H264: EngineSettings = EngineSettings {
        codec: Codec::H264(H264Preset::Ultrafast),
        fps: 30,
        kbps: 2500,
        threads: 1,
    };

    /// The engine's stand-in, which counts how often it is opened and
    /// refuses while `refuse` says so.
    fn standin(opened: Arc<AtomicU32>, refuse: Arc<AtomicU32>) -> Box<OpenHardware> {
        Box::new(move |config| {
            opened.fetch_add(1, Ordering::Relaxed);
            if refuse.load(Ordering::Relaxed) > 0 {
                return Err("busy".into());
            }
            HardwareEncoder::new(StandinDevice::new(), config)
                .map(|encoder| Box::new(encoder) as Box<dyn Hardware>)
                .map_err(|error| error.to_string())
        })
    }

    /// A picture that differs from the one before.
    fn picture(index: usize) -> I420Frame {
        I420Frame {
            width: 320,
            height: 240,
            y: (0..320 * 240)
                .map(|value| (value + index * 7) as u8)
                .collect(),
            u: vec![128; 160 * 120],
            v: vec![128; 160 * 120],
        }
    }

    /// The stream as the encoder's loop runs it: the server's choices go
    /// both ways between frames, every switch is followed by a key frame,
    /// the packets' sequence and the frame socket's records go on, and no
    /// failure is counted.
    #[test]
    fn the_stream_moves_between_the_engines_as_the_server_chooses() {
        let (opened, refuse) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let mut choice = Choice::new(
            Some(standin(Arc::clone(&opened), Arc::clone(&refuse))),
            Some(Box::new(|| Ok(()))),
        );
        assert_eq!(choice.available(), Some(true));
        let mut engine = Engine::with_hardware(H264, 320, 240, 7, 100, choice.opener()).unwrap();
        choice.opened(&engine, true);
        assert_eq!(engine.encoder_name(), "hardware");
        let mut numbering = Numbering::new(9);
        let mut sequence = engine.next_sequence();
        let mut records = Vec::new();
        let mut lines = 0;
        for index in 0..70 {
            // The server's choice changes every tenth frame.
            let switch = index > 0 && index % 10 == 0;
            if switch {
                lines += 1;
                let chosen = if (index / 10) % 2 == 1 {
                    EngineChoice::Software
                } else {
                    EngineChoice::Hardware
                };
                choice.take(lines, Some(chosen), Some(&engine));
                choice.apply(&mut engine).unwrap();
                let expected = match chosen {
                    EngineChoice::Software => "x264",
                    EngineChoice::Hardware => "hardware",
                };
                assert_eq!(engine.encoder_name(), expected, "frame {index}");
            }
            // The loop asks for a key frame after every choice.
            let (packets, frames) = engine
                .encode_frames(&picture(index), index as i64 * 33_333, index == 0 || switch)
                .unwrap();
            assert!(!packets.is_empty(), "frame {index}");
            assert_eq!(
                u16::from_be_bytes([packets[0][2], packets[0][3]]),
                sequence,
                "frame {index}"
            );
            sequence = engine.next_sequence();
            if switch {
                assert!(frames[0].key_frame, "frame {index}");
            }
            for frame in &frames {
                let header = numbering
                    .next(
                        frame.key_frame,
                        320,
                        240,
                        frame.data.len(),
                        index as u64 * 33_333,
                    )
                    .unwrap();
                // A key frame refers to itself.
                assert_eq!(header.reference == header.frame, frame.key_frame);
                records.push(header);
            }
        }
        // One record a frame, one stream, numbered on.
        assert_eq!(records.len(), 70);
        assert!(records.iter().all(|header| header.stream == 9));
        assert!(
            records
                .windows(2)
                .all(|pair| pair[1].frame == pair[0].frame + 1)
        );
        // The engine was opened for the stream and for each of the three
        // choices of it; nothing failed, and it can still be had.
        assert_eq!(opened.load(Ordering::Relaxed), 4);
        assert_eq!(engine.hardware_counters().unwrap().failures, 0);
        assert_eq!(choice.available(), Some(true));
        assert_eq!(choice.reason(), None);
    }

    /// An engine that cannot be had leaves x264 encoding with the reason,
    /// is not tried again at every frame, and is tried again when the
    /// choice is made again or a new stream opens.
    #[test]
    fn an_engine_that_cannot_be_had_leaves_x264_with_the_reason() {
        let (opened, refuse) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let looks = Arc::new(AtomicU32::new(0));
        let probe_looks = Arc::clone(&looks);
        let probe_refuses = Arc::clone(&refuse);
        let mut choice = Choice::new(
            Some(standin(Arc::clone(&opened), Arc::clone(&refuse))),
            Some(Box::new(move || {
                probe_looks.fetch_add(1, Ordering::Relaxed);
                match probe_refuses.load(Ordering::Relaxed) {
                    0 => Ok(()),
                    _ => Err("no such device".into()),
                }
            })),
        );
        assert_eq!(looks.load(Ordering::Relaxed), 1);
        // x264 is chosen before the first stream: no engine is opened, the
        // device is looked at again.
        choice.take(1, Some(EngineChoice::Software), None);
        assert_eq!(looks.load(Ordering::Relaxed), 2);
        assert!(choice.opener().is_none());
        let mut engine = Engine::with_hardware(H264, 320, 240, 7, 100, choice.opener()).unwrap();
        choice.opened(&engine, false);
        assert_eq!(engine.encoder_name(), "x264");
        // The engine is chosen while it is held elsewhere.
        refuse.store(1, Ordering::Relaxed);
        choice.take(2, Some(EngineChoice::Hardware), Some(&engine));
        assert_eq!(looks.load(Ordering::Relaxed), 2, "the switch answers");
        choice.apply(&mut engine).unwrap();
        assert_eq!(engine.encoder_name(), "x264");
        assert_eq!(choice.available(), Some(false));
        assert_eq!(choice.reason(), Some("busy"));
        for _ in 0..5 {
            choice.apply(&mut engine).unwrap();
        }
        assert_eq!(opened.load(Ordering::Relaxed), 1, "tried once");
        // A new stream tries again, and so does the choice made again.
        let next = engine.next_sequence();
        engine = Engine::with_hardware(H264, 640, 480, 7, next, choice.opener()).unwrap();
        choice.opened(&engine, true);
        assert_eq!(opened.load(Ordering::Relaxed), 2);
        assert_eq!(engine.encoder_name(), "x264");
        choice.apply(&mut engine).unwrap();
        assert_eq!(opened.load(Ordering::Relaxed), 2);
        refuse.store(0, Ordering::Relaxed);
        choice.take(3, Some(EngineChoice::Hardware), Some(&engine));
        choice.apply(&mut engine).unwrap();
        assert_eq!(engine.encoder_name(), "hardware");
        assert_eq!((choice.available(), choice.reason()), (Some(true), None));
        // A reason is cut for the report.
        assert_eq!(short_reason(&"x".repeat(500)).len(), MAX_REASON_CHARS);
    }

    /// An engine that failed on a stream is not tried again by the next
    /// stream, only when the choice is made again.
    #[test]
    fn an_engine_given_up_waits_for_the_choice_made_again() {
        let (opened, refuse) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let mut choice = Choice::new(Some(standin(Arc::clone(&opened), refuse)), None);
        assert_eq!(choice.available(), None, "not looked at");
        let mut engine = Engine::with_hardware(H264, 320, 240, 7, 100, choice.opener()).unwrap();
        choice.opened(&engine, true);
        assert_eq!(choice.available(), Some(true));
        // As the loop sees it when the engine gave up.
        choice.given_up(&engine);
        assert_eq!(choice.available(), Some(false));
        assert!(choice.opener().is_none());
        choice.apply(&mut engine).unwrap();
        // A viewer's connection names the same choice: no choice made.
        choice.take(0, Some(EngineChoice::Hardware), Some(&engine));
        assert!(choice.opener().is_none());
        choice.take(1, Some(EngineChoice::Hardware), Some(&engine));
        assert!(choice.opener().is_some());
        // Without a device the choice changes nothing.
        let mut nothing = Choice::new(None, None);
        nothing.take(1, Some(EngineChoice::Hardware), Some(&engine));
        assert!(nothing.opener().is_none());
        let mut x264 = Engine::new(H264, 320, 240, 7, 100).unwrap();
        nothing.apply(&mut x264).unwrap();
        assert_eq!(x264.encoder_name(), "x264");
        assert_eq!(
            (nothing.available(), nothing.wanted()),
            (None, EngineChoice::Hardware)
        );
        let _ = opened;
    }
}
