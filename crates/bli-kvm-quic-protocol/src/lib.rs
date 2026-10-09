//! Messages of the QUIC transport: between the page and the gateway, between
//! the gateway and the web server, and the header of an encoded frame.
//! See `docs/design/quic-transport-v1.md`.
//!
//! Every message is bounded, and a length is checked before memory is taken
//! for what it announces. An unknown type or field, a reserved value that is
//! not zero and a size out of range are refused. One protocol version exists:
//! the page, the gateway and the server are updated together, and each checks
//! the other's number when it connects.

#![forbid(unsafe_code)]

pub mod frame;
pub mod link;
pub mod page;

/// The version every side sends first and requires of the other. Version 2
/// sends a group of pictures (a key frame and its delta frames) on a stream
/// of its own instead of the delta frames as datagrams.
pub const PROTOCOL: u16 = 2;
