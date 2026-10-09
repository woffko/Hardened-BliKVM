//! The link between the gateway and the web server: a Unix stream socket of
//! the server, one connection, each message a JSON object preceded by its
//! length as four bytes, most significant first. Encoded frames never travel
//! here.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

/// A link message, without its four length bytes.
pub const MAX_LINK_MESSAGE_BYTES: usize = 64 * 1024;

/// The least share of the chosen bitrate, in percent, that a session asks
/// for in [`GatewayToServer::VideoShare`]: below it the picture is not worth
/// having.
pub const LEAST_VIDEO_SHARE: u8 = 25;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "t",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum GatewayToServer {
    /// First on the link, and again after the certificate was replaced.
    Hello {
        protocol: u16,
        /// SHA-256 of the certificate the endpoint presents, hexadecimal.
        certificate_sha256: String,
        /// The certificate's validity, seconds since 1970.
        not_before: u64,
        not_after: u64,
        /// The UDP port the endpoint listens on.
        port: u16,
    },
    /// A page presented this token; `peer` is its address, for the log.
    Redeem {
        id: u64,
        token: String,
        peer: String,
    },
    /// A request of the HID lease API from a session's input stream.
    Lease {
        id: u64,
        session: String,
        request: serde_json::Value,
    },
    /// A session asks for, or gives up, the picture.
    Video {
        id: u64,
        session: String,
        subscribe: bool,
    },
    /// The share of the chosen bitrate, in percent, that a session's path
    /// carries, whenever it changes (`docs/design/quic-transport-v1.md`,
    /// "The bitrate"). The server runs the H.264 encoder at the chosen
    /// bitrate times the least share of the sessions that watch.
    VideoShare { session: String, percent: u8 },
    /// A session's connection is gone.
    Closed { session: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedeemRefusal {
    /// Unknown, used or expired, or its web session has ended.
    Token,
    /// The web session already has as many QUIC sessions as it may.
    Sessions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// The web session ended: logout, expiry, lock, a changed password.
    Session,
    /// The server is shutting down.
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "t",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ServerToGateway {
    /// The answer to the gateway's hello: the server's protocol number. A
    /// gateway that reads another number than its own serves nobody.
    Hello { protocol: u16 },
    /// `session` is the reference the gateway uses from now on; the web
    /// session's own token never leaves the server.
    Redeemed {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refused: Option<RedeemRefusal>,
    },
    LeaseAnswer {
        id: u64,
        response: serde_json::Value,
    },
    VideoAnswer {
        id: u64,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// The server ends a session.
    End { session: String, reason: EndReason },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LinkError {
    #[error("a link message of {0} bytes is out of range")]
    Size(usize),
    #[error("the link message is not valid: {0}")]
    Invalid(String),
}

/// A message with its four length bytes in front.
pub fn encode<T: Serialize>(message: &T) -> Result<Vec<u8>, LinkError> {
    let body =
        serde_json::to_vec(message).map_err(|error| LinkError::Invalid(error.to_string()))?;
    if body.is_empty() || body.len() > MAX_LINK_MESSAGE_BYTES {
        return Err(LinkError::Size(body.len()));
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// The length a message's four length bytes announce, if it may be read.
pub fn body_length(prefix: [u8; 4]) -> Result<usize, LinkError> {
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 || length > MAX_LINK_MESSAGE_BYTES {
        return Err(LinkError::Size(length));
    }
    Ok(length)
}

pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, LinkError> {
    if body.is_empty() || body.len() > MAX_LINK_MESSAGE_BYTES {
        return Err(LinkError::Size(body.len()));
    }
    serde_json::from_slice(body).map_err(|error| LinkError::Invalid(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_messages_round_trip_with_their_documented_names() {
        let hello = GatewayToServer::Hello {
            protocol: 1,
            certificate_sha256: "ab".repeat(32),
            not_before: 10,
            not_after: 20,
            port: 443,
        };
        let bytes = encode(&hello).unwrap();
        let length = body_length([bytes[0], bytes[1], bytes[2], bytes[3]]).unwrap();
        assert_eq!(length, bytes.len() - 4);
        assert_eq!(decode::<GatewayToServer>(&bytes[4..]), Ok(hello));
        let text = std::str::from_utf8(&bytes[4..]).unwrap();
        assert!(
            text.contains(r#""certificateSha256""#) && text.contains(r#""notBefore":10"#),
            "{text}"
        );

        let share = serde_json::to_string(&GatewayToServer::VideoShare {
            session: "s".into(),
            percent: 75,
        })
        .unwrap();
        assert_eq!(share, r#"{"t":"video_share","session":"s","percent":75}"#);
        assert_eq!(
            decode::<GatewayToServer>(share.as_bytes()),
            Ok(GatewayToServer::VideoShare {
                session: "s".into(),
                percent: 75,
            })
        );
        assert!(
            decode::<GatewayToServer>(br#"{"t":"video_share","session":"s","percent":300}"#)
                .is_err()
        );

        let redeemed = serde_json::to_string(&ServerToGateway::Redeemed {
            id: 4,
            session: None,
            refused: Some(RedeemRefusal::Sessions),
        })
        .unwrap();
        assert_eq!(redeemed, r#"{"t":"redeemed","id":4,"refused":"sessions"}"#);
        let end = serde_json::to_string(&ServerToGateway::End {
            session: "s".into(),
            reason: EndReason::Session,
        })
        .unwrap();
        assert_eq!(end, r#"{"t":"end","session":"s","reason":"session"}"#);
    }

    #[test]
    fn lengths_and_shapes_out_of_range_are_refused() {
        assert_eq!(body_length([0, 0, 0, 0]), Err(LinkError::Size(0)));
        assert_eq!(body_length([0, 1, 0, 1]), Err(LinkError::Size(65537)));
        assert_eq!(body_length([0, 1, 0, 0]), Ok(65536));
        assert!(decode::<ServerToGateway>(br#"{"t":"hello","protocol":1,"more":2}"#).is_err());
        assert!(decode::<ServerToGateway>(br#"{"t":"reboot"}"#).is_err());
        assert!(decode::<GatewayToServer>(b"").is_err());
    }
}
