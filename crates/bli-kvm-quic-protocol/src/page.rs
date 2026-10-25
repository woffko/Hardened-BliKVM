//! The control and input streams between the page and the gateway.
//!
//! Each message is a JSON object preceded by its length as two bytes, most
//! significant first.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

/// The only path the gateway serves.
pub const SESSION_PATH: &str = "/kvm";
/// A control or input message, without its two length bytes.
pub const MAX_MESSAGE_BYTES: usize = 4096;
/// A session token: 32 random bytes as lower-case hexadecimal digits.
pub const TOKEN_CHARACTERS: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "t",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PageControl {
    /// First on the control stream.
    Hello {
        protocol: u16,
        token: String,
    },
    /// The page's clock in milliseconds, returned unchanged.
    Ping {
        id: u32,
        page_time: f64,
    },
    /// A frame of a group arrived whole: its capture time, said as it
    /// arrives, for the gateway to see what is still on its way. It is not
    /// answered.
    Received {
        capture: u64,
    },
    Video {
        subscribe: bool,
    },
    /// A frame was lost: the stream it belonged to, the last frame that was
    /// decoded, and how many were given up since the last request.
    Keyframe {
        stream: u32,
        last_frame: u32,
        dropped: u32,
    },
    Bye {},
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// The first message was not a hello, or came too late.
    Hello,
    Protocol,
    /// The token is unknown, used or expired, or its web session has ended.
    Token,
    /// The web session already has as many QUIC sessions as it may.
    Sessions,
    /// The server cannot be asked right now.
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoState {
    Started,
    Stopped,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Closing {
    /// The web session ended: logout, expiry, lock, a changed password.
    Session,
    /// The board's switch went off or the server restarted.
    Shutdown,
    Protocol,
    Idle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "t",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum GatewayControl {
    Welcome {
        protocol: u16,
        /// The largest datagram the gateway sends on this path.
        max_datagram: u16,
    },
    Refused {
        reason: Refusal,
    },
    /// `board_time` is the board's monotonic clock in microseconds when the
    /// ping was read.
    Pong {
        id: u32,
        page_time: f64,
        board_time: u64,
    },
    Video {
        state: VideoState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stream: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// The connection as the gateway sees it: the round-trip time, packets
    /// the transport gave up as lost, what the gateway sent or had to drop
    /// of the picture, and the share of the chosen bitrate, in percent, that
    /// the session's path carries (`docs/design/quic-transport-v1.md`, "The
    /// bitrate").
    Counters {
        round_trip_micros: u32,
        packets_lost: u64,
        frames_sent: u64,
        frames_dropped: u64,
        key_frames: u64,
        bitrate_share: u8,
    },
    Closing {
        reason: Closing,
    },
}

/// A request of the HID lease API with the page's sequence number; `request`
/// is what the page would send to `/api/hid/lease`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputRequest {
    pub seq: u32,
    pub request: serde_json::Value,
}

/// The lease API's answer to the request with the same number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputResponse {
    pub seq: u32,
    pub response: serde_json::Value,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MessageError {
    #[error("a message of {0} bytes is out of range")]
    Size(usize),
    #[error("the message is not valid: {0}")]
    Invalid(String),
}

/// A message with its two length bytes in front.
pub fn encode<T: Serialize>(message: &T) -> Result<Vec<u8>, MessageError> {
    let body =
        serde_json::to_vec(message).map_err(|error| MessageError::Invalid(error.to_string()))?;
    if body.is_empty() || body.len() > MAX_MESSAGE_BYTES {
        return Err(MessageError::Size(body.len()));
    }
    let mut out = Vec::with_capacity(2 + body.len());
    out.extend_from_slice(&(body.len() as u16).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Collects the bytes of a stream and hands out its messages one by one. It
/// never holds more than one message's worth beyond what it was given.
#[derive(Debug, Default)]
pub struct MessageReader {
    buffer: Vec<u8>,
}

impl MessageReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// The next complete message, `None` while one is still arriving, or the
    /// reason the stream cannot go on.
    pub fn take<T: DeserializeOwned>(&mut self) -> Result<Option<T>, MessageError> {
        if self.buffer.len() < 2 {
            return Ok(None);
        }
        let length = usize::from(u16::from_be_bytes([self.buffer[0], self.buffer[1]]));
        if length == 0 || length > MAX_MESSAGE_BYTES {
            return Err(MessageError::Size(length));
        }
        if self.buffer.len() < 2 + length {
            return Ok(None);
        }
        let message = serde_json::from_slice(&self.buffer[2..2 + length])
            .map_err(|error| MessageError::Invalid(error.to_string()))?;
        self.buffer.drain(..2 + length);
        Ok(Some(message))
    }

    /// Bytes held that are not yet a complete message.
    pub fn pending(&self) -> usize {
        self.buffer.len()
    }
}

/// Whether a token has the shape the server issues.
pub fn token_is_well_formed(token: &str) -> bool {
    token.len() == TOKEN_CHARACTERS
        && token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_messages_have_the_documented_shape() {
        let hello = PageControl::Hello {
            protocol: 1,
            token: "ab".repeat(32),
        };
        let bytes = encode(&hello).unwrap();
        assert_eq!(
            usize::from(u16::from_be_bytes([bytes[0], bytes[1]])),
            bytes.len() - 2
        );
        let text = std::str::from_utf8(&bytes[2..]).unwrap();
        assert!(
            text.starts_with(r#"{"t":"hello","protocol":1,"token":""#),
            "{text}"
        );

        let received =
            serde_json::to_string(&PageControl::Received { capture: 1_234_567 }).unwrap();
        assert_eq!(received, r#"{"t":"received","capture":1234567}"#);

        let keyframe = serde_json::to_string(&PageControl::Keyframe {
            stream: 7,
            last_frame: 41,
            dropped: 2,
        })
        .unwrap();
        assert_eq!(
            keyframe,
            r#"{"t":"keyframe","stream":7,"lastFrame":41,"dropped":2}"#
        );

        let welcome = serde_json::to_string(&GatewayControl::Welcome {
            protocol: 1,
            max_datagram: 1200,
        })
        .unwrap();
        assert_eq!(
            welcome,
            r#"{"t":"welcome","protocol":1,"maxDatagram":1200}"#
        );
        let video = serde_json::to_string(&GatewayControl::Video {
            state: VideoState::Unavailable,
            stream: None,
            width: None,
            height: None,
            reason: Some("encoder".into()),
        })
        .unwrap();
        assert_eq!(
            video,
            r#"{"t":"video","state":"unavailable","reason":"encoder"}"#
        );
        let counters = serde_json::to_string(&GatewayControl::Counters {
            round_trip_micros: 40_000,
            packets_lost: 3,
            frames_sent: 300,
            frames_dropped: 2,
            key_frames: 6,
            bitrate_share: 75,
        })
        .unwrap();
        assert_eq!(
            counters,
            concat!(
                r#"{"t":"counters","roundTripMicros":40000,"packetsLost":3,"#,
                r#""framesSent":300,"framesDropped":2,"keyFrames":6,"bitrateShare":75}"#
            )
        );
    }

    #[test]
    fn unknown_types_and_fields_are_refused() {
        for text in [
            r#"{"t":"hello","protocol":1,"token":"x","extra":1}"#,
            r#"{"t":"shutdown"}"#,
            r#"{"protocol":1,"token":"x"}"#,
            r#"{"t":"ping","id":1}"#,
            r#"{"t":"ping","id":-1,"pageTime":1}"#,
            r#"{"t":"ping","id":1,"pageTime":1,"received":5}"#,
            r#"{"t":"received","capture":-1}"#,
            r#"[1,2,3]"#,
        ] {
            assert!(serde_json::from_str::<PageControl>(text).is_err(), "{text}");
        }
        assert!(serde_json::from_str::<InputRequest>(r#"{"seq":1,"request":{},"x":1}"#).is_err());
    }

    #[test]
    fn the_reader_returns_messages_as_they_complete() {
        let first = encode(&PageControl::Ping {
            id: 1,
            page_time: 10.5,
        })
        .unwrap();
        let second = encode(&PageControl::Bye {}).unwrap();
        let mut all = first.clone();
        all.extend_from_slice(&second);
        let mut reader = MessageReader::new();
        // One byte at a time: nothing until a message is whole.
        let mut seen = Vec::new();
        for byte in &all {
            reader.push(std::slice::from_ref(byte));
            while let Some(message) = reader.take::<PageControl>().unwrap() {
                seen.push(message);
            }
        }
        assert_eq!(
            seen,
            vec![
                PageControl::Ping {
                    id: 1,
                    page_time: 10.5,
                },
                PageControl::Bye {}
            ]
        );
        assert_eq!(reader.pending(), 0);
    }

    #[test]
    fn a_length_out_of_range_is_refused_before_the_body_arrives() {
        let mut reader = MessageReader::new();
        reader.push(&[0, 0]);
        assert_eq!(reader.take::<PageControl>(), Err(MessageError::Size(0)));
        let mut reader = MessageReader::new();
        reader.push(&((MAX_MESSAGE_BYTES + 1) as u16).to_be_bytes());
        assert_eq!(
            reader.take::<PageControl>(),
            Err(MessageError::Size(MAX_MESSAGE_BYTES + 1))
        );
        let oversized = PageControl::Hello {
            protocol: 1,
            token: "a".repeat(MAX_MESSAGE_BYTES),
        };
        assert!(matches!(encode(&oversized), Err(MessageError::Size(_))));
    }

    #[test]
    fn tokens_have_one_shape() {
        assert!(token_is_well_formed(&"0f".repeat(32)));
        assert!(!token_is_well_formed(&"0F".repeat(32)));
        assert!(!token_is_well_formed(&"0f".repeat(31)));
        assert!(!token_is_well_formed(&"0g".repeat(32)));
    }
}
