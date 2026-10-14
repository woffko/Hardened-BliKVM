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

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use frame::{Dropped, FRAME_BYTES, FrameError, MAX_MESSAGE_BYTES};

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

/// What one side may ask the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Call {
    /// Answered at once with a pong: the round trip.
    Ping,
}

/// The answer to a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    Pong,
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
}

impl Hello {
    /// This side's hello.
    pub fn new(program: &str, version: &str, commit: &str) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            program: program.to_owned(),
            version: version.to_owned(),
            commit: commit.to_owned(),
        }
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
