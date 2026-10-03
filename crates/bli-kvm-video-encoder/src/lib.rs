//! Codec-independent core of the H616 VP8 WebRTC video pipeline.
//!
//! The capture stays owned by uStreamer; this crate only decides which of its
//! MJPEG frames are worth encoding, keeps at most one pending frame, and
//! packetizes encoded VP8 frames into RTP (RFC 7741) for a loopback consumer.
//! JPEG decoding and VP8 encoding are separate, replaceable stages.

#[cfg(feature = "h264")]
pub mod choice;
pub mod demand;
#[cfg(feature = "h264")]
pub mod engine;
pub mod frames;
pub mod gate;
#[cfg(feature = "h264")]
pub mod h264;
pub mod mjpeg;
pub mod report;
pub mod rtp;
pub mod slot;
#[cfg(feature = "turbojpeg")]
pub mod turbo;
#[cfg(feature = "vp8")]
pub mod vp8;
pub mod yuv;

pub use gate::{FrameDecision, FrameGate, GatePolicy};
pub use rtp::{H264Packetizer, RtpError, Vp8Packetizer};
pub use slot::LatestFrame;
