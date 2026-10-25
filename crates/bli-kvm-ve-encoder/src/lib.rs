//! The H616 video engine's H.264 encoder as a client of its driver sees it
//! (`docs/design/h616-ve-encoder-driver-v1.md`): the driver encodes a
//! picture at a QP and nothing more, and everything a stream needs besides
//! is here. The headers and their bits; the rate control, which gives every
//! frame its QP and chooses between the engine's two searches for motion;
//! the frame that says "nothing changed"; the requests to the driver.
//!
//! What is known about the engine, and how it was learnt, is in
//! `docs/design/h616-ve-h264-encoder.md`.

pub mod abi;
pub mod bits;
pub mod device;
pub mod driver;
pub mod encoder;
pub mod headers;
pub mod nv12;
pub mod policy;
pub mod rate;
pub mod sets;
pub mod skip;
#[cfg(feature = "standin")]
pub mod standin;
