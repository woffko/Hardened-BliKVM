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

