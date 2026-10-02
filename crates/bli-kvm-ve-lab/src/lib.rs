//! The lab tool of the hardware H.264 plan
//! (`docs/baselines/work-plan-20261002-hardware-h264.md`): test pictures,
//! the stream's headers, the engine's registers as the open sources describe
//! them, the plan of an encode, and a stand-in for the engine in software.
//!
//! It is a lab instrument: no package, slot or image contains it.

pub mod bits;
pub mod device;
pub mod engine;
pub mod headers;
pub mod lab;
pub mod picture;
pub mod psnr;
pub mod rate;
pub mod regs;
pub mod session;
pub mod standin;
