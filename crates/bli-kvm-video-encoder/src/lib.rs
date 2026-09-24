//! Codec-independent core of the H616 VP8 WebRTC video pipeline.
//!
//! The capture stays owned by uStreamer; this crate only decides which of its
//! MJPEG frames are worth encoding, keeps at most one pending frame, and
//! packetizes encoded VP8 frames into RTP (RFC 7741) for a loopback consumer.
//! JPEG decoding and VP8 encoding are separate, replaceable stages.

pub mod gate;
pub mod rtp;
pub mod slot;

pub use gate::{FrameDecision, FrameGate, GatePolicy};
pub use rtp::{RtpError, Vp8Packetizer};
pub use slot::LatestFrame;
