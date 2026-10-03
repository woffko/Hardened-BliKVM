//! The lab tool of the hardware H.264 plan
//! (`docs/baselines/work-plan-20261002-hardware-h264.md`): test pictures,
//! the stream's headers, the engine's registers as the open sources describe
//! them, the plan of an encode, and a stand-in for the engine in software.
//!
//! It is a lab instrument: no package, slot or image contains it.

pub mod device;
pub mod driver;
pub mod engine;
pub mod lab;
pub mod picture;
pub mod psnr;
pub mod regs;
pub mod session;

// The product's own: the lab measures the engine with the headers and the
// rate control the product's encoder runs.
pub use bli_kvm_ve_encoder::{bits, headers, policy, rate};

/// The stand-in for the engine, and the frame that says "nothing changed".
pub mod standin {
    pub use bli_kvm_ve_encoder::{skip::unchanged_frame, standin::slice_data};
}
