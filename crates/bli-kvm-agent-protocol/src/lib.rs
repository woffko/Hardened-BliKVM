//! The messages between the Hardened BliKVM board and its agent on the target
//! (`docs/baselines/work-plan-20261014-target-agent.md`).
//!
//! While the operator's "Target agent" switch is on, the board's absolute
//! mouse interface carries a second, vendor-defined collection: report 2,
//! 64 bytes with its ID, each way. The agent on the target opens that
//! collection. Each side writes messages as JSON in frames of those reports
//! (`frame`). The agent begins with a hello, the board answers with its own
//! or refuses with a reason; then either side calls the other (a request with
//! a number and its answer). Both ends are built from this crate.

#![forbid(unsafe_code)]

pub mod frame;
pub mod reads;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use frame::{Dropped, FRAME_BYTES, FrameError, MAX_MESSAGE_BYTES};
pub use reads::{Keyboard, Layout, Monitor, System};

/// The agent's report ID in the absolute mouse's interface (the mouse's own
/// reports are 1 while the agent's collection is there).
pub const AGENT_REPORT_ID: u8 = 2;
/// One of the agent's reports, its ID first.
pub const REPORT_BYTES: usize = FRAME_BYTES + 1;

/// The protocol's version, said in the hello: the other side refuses
/// another.
pub const PROTOCOL_VERSION: u32 = 1;
/// The longest name, version or commit a hello may carry.
pub const MAX_HELLO_TEXT: usize = 64;

/// One message, either way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// Who is speaking: the agent's first message, and the board's answer.
    Hello(Hello),
    /// The other side's hello is not taken, and why; nothing more follows
    /// until a new hello.
    Refused { reason: String },
    /// A call to the other side.
    Request { id: u32, call: Call },
    /// The answer to the request of this number.
    Answer { id: u32, answer: Answer },
    /// The request of this number was not done, and why.
    Failed { id: u32, reason: String },
}

/// What one side may ask the other. Either side answers a ping; the
/// others are the board's to the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Call {
    /// Answered at once with a pong: the round trip.
    Ping,
    /// The target's system and session, as the hello says them, now.
    System,
    /// The active monitors.
    Monitors,
    /// The keyboard layouts.
    Keyboard,
}

/// The answer to a call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    Pong,
    System(System),
    Monitors(Vec<Monitor>),
    Keyboard(Keyboard),
}

impl Answer {
    /// Whether the answer is within its bounds.
    pub fn is_valid(&self) -> bool {
        match self {
            Answer::Pong => true,
            Answer::System(system) => system.is_valid(),
            Answer::Monitors(monitors) => reads::monitors_are_valid(monitors),
            Answer::Keyboard(keyboard) => keyboard.is_valid(),
        }
    }

    /// Whether the answer answers `call` and is within its bounds.
    pub fn answers(&self, call: Call) -> bool {
        let fits = matches!(
            (call, self),
            (Call::Ping, Answer::Pong)
                | (Call::System, Answer::System(_))
                | (Call::Monitors, Answer::Monitors(_))
                | (Call::Keyboard, Answer::Keyboard(_))
        );
        fits && self.is_valid()
    }
}

/// Who is speaking. Unknown fields are ignored, so that a side of another
/// version can read the hello and refuse it with a reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    /// The program: the board's owner or the agent.
    pub program: String,
    pub version: String,
    pub commit: String,
    /// The agent's system, which the board's hello leaves out. One of
    /// another shape is read as one out of bounds, so that the hello is
    /// still read and refused with a reason.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "system_or_unreadable"
    )]
    pub system: Option<System>,
}

fn system_or_unreadable<'de, D>(deserializer: D) -> Result<Option<System>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.map(|value| {
        serde_json::from_value(value).unwrap_or_else(|_| System {
            product: String::new(),
            release: String::new(),
            build: 0,
            revision: 0,
            architecture: "unknown".into(),
            locked: false,
        })
    }))
}

impl Hello {
    /// This side's hello.
    pub fn new(program: &str, version: &str, commit: &str) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            program: program.to_owned(),
            version: version.to_owned(),
            commit: commit.to_owned(),
            system: None,
        }
    }

    /// This side's hello with its system.
    pub fn with_system(mut self, system: System) -> Self {
        self.system = Some(system);
        self
    }

    /// Why the other side's hello is not taken, if it is not.
    pub fn refusal(&self) -> Option<String> {
        if self.protocol != PROTOCOL_VERSION {
            return Some(format!(
                "protocol {} is not this side's {PROTOCOL_VERSION}",
                self.protocol
            ));
        }
        for (name, text) in [
            ("program", &self.program),
            ("version", &self.version),
            ("commit", &self.commit),
        ] {
            if text.is_empty()
                || text.chars().count() > MAX_HELLO_TEXT
                || text.chars().any(char::is_control)
            {
                return Some(format!(
                    "the hello's {name} is empty, longer than {MAX_HELLO_TEXT} characters or has a control character"
                ));
            }
        }
        if self
            .system
            .as_ref()
            .is_some_and(|system| !system.is_valid())
        {
            return Some("the hello's system is out of its bounds".into());
        }
        None
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EncodeError {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error("the message is not JSON: {0}")]
    Json(String),
}

/// Why nothing came of a report.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReadError {
    #[error("a report dropped: {0:?}")]
    Dropped(Dropped),
    #[error("a message that is not one of the protocol's: {0}")]
    Undecodable(String),
}

/// One side's writing: messages numbered in turn, cut into reports.
#[derive(Debug, Default)]
pub struct Writer {
    next: u16,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The reports that carry `message`, to be written in order.
    pub fn reports(&mut self, message: &Message) -> Result<Vec<[u8; REPORT_BYTES]>, EncodeError> {
        let bytes =
            serde_json::to_vec(message).map_err(|error| EncodeError::Json(error.to_string()))?;
        let frames = frame::frames(self.next, &bytes)?;
        self.next = self.next.wrapping_add(1);
        Ok(frames
            .iter()
            .map(|frame| {
                let mut report = [0_u8; REPORT_BYTES];
                report[0] = AGENT_REPORT_ID;
                report[1..].copy_from_slice(frame);
                report
            })
            .collect())
    }
}

/// One side's reading: reports in, messages out.
#[derive(Debug, Default)]
pub struct Reader {
    assembler: frame::Assembler,
}

impl Reader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes one report, its ID first: a message once its last frame came.
    pub fn push(&mut self, report: &[u8]) -> Result<Option<Message>, ReadError> {
        let frame = match report.split_first() {
            Some((&AGENT_REPORT_ID, frame)) => frame,
            _ => return Err(ReadError::Dropped(Dropped::NotAFrame)),
        };
        match self.assembler.push(frame).map_err(ReadError::Dropped)? {
            None => Ok(None),
            Some((_, bytes)) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| ReadError::Undecodable(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(message: &Message) -> Message {
        let mut writer = Writer::new();
        let mut reader = Reader::new();
        let reports = writer.reports(message).unwrap();
        let (last, rest) = reports.split_last().unwrap();
        for report in rest {
            assert_eq!(reader.push(report), Ok(None));
        }
        reader.push(last).unwrap().unwrap()
    }

    #[test]
    fn every_message_comes_back_as_it_was_sent() {
        for message in [
            Message::Hello(Hello::new("hardened-blikvm-agent", "0.1.0", "3c940a7e681c")),
            Message::Refused {
                reason: "protocol 2 is not this side's 1".into(),
            },
            Message::Request {
                id: u32::MAX,
                call: Call::Ping,
            },
            Message::Answer {
                id: 7,
                answer: Answer::Pong,
            },
            Message::Failed {
                id: 8,
                reason: "Ünïcode and \"quotes\"".into(),
            },
        ] {
            assert_eq!(round_trip(&message), message);
        }
    }

    #[test]
    fn the_wire_is_pinned() {
        assert_eq!(
            serde_json::to_string(&Message::Request {
                id: 1,
                call: Call::Ping
            })
            .unwrap(),
            r#"{"type":"request","id":1,"call":"ping"}"#
        );
        assert_eq!(
            serde_json::to_string(&Message::Answer {
                id: 1,
                answer: Answer::Pong
            })
            .unwrap(),
            r#"{"type":"answer","id":1,"answer":"pong"}"#
        );
        // A ping and its pong fit in one report each, whatever their number.
        let mut writer = Writer::new();
        for message in [
            Message::Request {
                id: u32::MAX,
                call: Call::Ping,
            },
            Message::Answer {
                id: u32::MAX,
                answer: Answer::Pong,
            },
        ] {
            assert_eq!(writer.reports(&message).unwrap().len(), 1);
        }
    }

    #[test]
    fn the_reads_go_both_ways_and_answer_only_their_calls() {
        let system = System {
            product: "Windows 11 Pro".into(),
            release: "24H2".into(),
            build: 26100,
            revision: 2033,
            architecture: "x86_64".into(),
            locked: false,
        };
        let monitors = vec![Monitor {
            name: "DELL U2719D".into(),
            maker: "DEL".into(),
            product: 0xd0a5,
            x: -2560,
            y: 0,
            width: 2560,
            height: 1440,
            refresh_millihertz: 59_951,
            primary: false,
            scale: 100,
            board: true,
        }];
        let keyboard = Keyboard {
            current: None,
            installed: vec![Layout {
                id: "04090409".into(),
                language: "en-US".into(),
                name: "English (United States) - US".into(),
            }],
        };
        for (call, answer) in [
            (Call::System, Answer::System(system.clone())),
            (Call::Monitors, Answer::Monitors(monitors.clone())),
            (Call::Keyboard, Answer::Keyboard(keyboard)),
        ] {
            assert!(answer.answers(call), "{call:?}");
            assert!(!answer.answers(Call::Ping));
            let message = Message::Answer { id: 9, answer };
            assert_eq!(round_trip(&message), message);
        }
        let mut odd = monitors;
        odd[0].scale = 50;
        assert!(!Answer::Monitors(odd).answers(Call::Monitors));
        assert_eq!(
            serde_json::to_string(&Message::Request {
                id: 2,
                call: Call::Monitors
            })
            .unwrap(),
            r#"{"type":"request","id":2,"call":"monitors"}"#
        );

        // The agent's hello carries its system; the board's leaves it out.
        let hello = Hello::new("hardened-blikvm-agent", "0.1.0", "abc").with_system(system.clone());
        assert_eq!(
            round_trip(&Message::Hello(hello.clone())),
            Message::Hello(hello.clone())
        );
        assert_eq!(hello.refusal(), None);
        assert!(
            !serde_json::to_string(&Hello::new("board", "0.1.0", "abc"))
                .unwrap()
                .contains("system")
        );
        let mut odd = system;
        odd.product = "\u{1b}[2J".into();
        assert!(
            Hello::new("agent", "0.1.0", "abc")
                .with_system(odd)
                .refusal()
                .is_some()
        );
        // A system of another shape in a hello of this protocol is refused.
        let other: Message = serde_json::from_str(
            r#"{"type":"hello","protocol":1,"program":"a","version":"1","commit":"c","system":{"windows":"11"}}"#,
        )
        .unwrap();
        let Message::Hello(other) = other else {
            panic!("{other:?}");
        };
        assert_eq!(
            other.refusal().as_deref(),
            Some("the hello's system is out of its bounds")
        );
    }

    #[test]
    fn a_hello_of_another_version_is_read_and_refused_with_a_reason() {
        let newer = br#"{"type":"hello","protocol":2,"program":"hardened-blikvm-agent","version":"0.2.0","commit":"abc","system":{"windows":"11"}}"#;
        let message: Message = serde_json::from_slice(newer).unwrap();
        let Message::Hello(hello) = message else {
            panic!("{message:?}");
        };
        assert_eq!(
            hello.refusal().as_deref(),
            Some("protocol 2 is not this side's 1")
        );
        assert_eq!(Hello::new("agent", "0.1.0", "unknown").refusal(), None);
    }

    #[test]
    fn a_hello_with_odd_text_is_refused() {
        for (program, version, commit) in [
            ("", "0.1.0", "abc"),
            ("agent", &"9".repeat(MAX_HELLO_TEXT + 1) as &str, "abc"),
            ("agent", "0.1.0", "abc\u{1b}[2J"),
        ] {
            assert!(Hello::new(program, version, commit).refusal().is_some());
        }
        assert_eq!(
            Hello::new("agent", &"9".repeat(MAX_HELLO_TEXT), "abc").refusal(),
            None
        );
    }

    #[test]
    fn a_message_that_is_not_the_protocols_is_undecodable() {
        let mut reader = Reader::new();
        for bytes in [
            &br#"{"type":"request","id":1,"call":"format_disk"}"#[..],
            br#"{"type":"answer"}"#,
            b"not json",
        ] {
            let mut result = Ok(None);
            for frame in frame::frames(1, bytes).unwrap() {
                let mut report = vec![AGENT_REPORT_ID];
                report.extend_from_slice(&frame);
                result = reader.push(&report);
            }
            assert!(
                matches!(result, Err(ReadError::Undecodable(_))),
                "{result:?}"
            );
        }
    }

    #[test]
    fn reports_carry_the_agents_id_and_others_are_refused() {
        let mut writer = Writer::new();
        let mut reader = Reader::new();
        let ping = Message::Request {
            id: 3,
            call: Call::Ping,
        };
        let mut report = writer.reports(&ping).unwrap()[0];
        assert_eq!(report.len(), 64);
        assert_eq!(report[0], AGENT_REPORT_ID);
        // The mouse's report 1, a report without its ID and nothing at all.
        let mouse = [1, 0, 0, 0x40, 0, 0x40, 0, 0];
        assert_eq!(
            reader.push(&mouse),
            Err(ReadError::Dropped(Dropped::NotAFrame))
        );
        assert_eq!(
            reader.push(&report[1..]),
            Err(ReadError::Dropped(Dropped::NotAFrame))
        );
        assert_eq!(
            reader.push(&[]),
            Err(ReadError::Dropped(Dropped::NotAFrame))
        );
        assert_eq!(reader.push(&report), Ok(Some(ping)));
        report[0] = 1;
        assert_eq!(
            reader.push(&report),
            Err(ReadError::Dropped(Dropped::NotAFrame))
        );
    }

    #[test]
    fn messages_are_numbered_in_turn_and_the_number_wraps() {
        let mut writer = Writer { next: u16::MAX };
        let ping = Message::Request {
            id: 1,
            call: Call::Ping,
        };
        let first = writer.reports(&ping).unwrap();
        let second = writer.reports(&ping).unwrap();
        assert_eq!(&first[0][3..5], &u16::MAX.to_le_bytes());
        assert_eq!(&second[0][3..5], &0_u16.to_le_bytes());
    }

    #[test]
    fn a_message_too_long_is_not_written() {
        let mut writer = Writer::new();
        let failed = Message::Failed {
            id: 1,
            reason: "x".repeat(MAX_MESSAGE_BYTES),
        };
        assert!(matches!(
            writer.reports(&failed),
            Err(EncodeError::Frame(FrameError::TooLong(_)))
        ));
    }
}
