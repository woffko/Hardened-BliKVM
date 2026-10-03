//! What the encoder asks of the engine: the driver, or what stands in for
//! it in a test. One request is one pass of the engine over a picture.

use std::{io, time::Duration};

use crate::{headers::RawUnit, rate::Search};

/// Which pass over the picture before is the frame a new picture refers
/// to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reference {
    /// The last one.
    Last,
    /// The one before the last.
    Earlier,
    /// None of them: no pass over that picture was sent, and the stream
    /// stays where it was.
    Stay,
}

/// The picture of a pass.
#[derive(Debug, Clone, Copy)]
pub enum Picture<'a> {
    /// A new one: NV12 of whole macroblocks, and what it refers to.
    New {
        nv12: &'a [u8],
        reference: Reference,
    },
    /// The one given last, once more, against the same reference.
    Again,
}

#[derive(Debug, Clone, Copy)]
pub struct Pass<'a> {
    pub key: bool,
    pub qp: u32,
    pub search: Search,
    pub picture: Picture<'a>,
}

/// What a pass says of itself besides its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Made {
    /// The encoder's status bits as the interrupt found them.
    pub status: u32,
    /// From the start trigger to the interrupt.
    pub time: Duration,
    /// The engine's numbers about the frame.
    pub statistics: [u32; 3],
}

/// The sizes of a configured stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sizes {
    /// The bytes of a picture: NV12 of whole macroblocks.
    pub picture_bytes: usize,
    /// The most a frame can have.
    pub stream_bytes: usize,
}

pub trait Device {
    /// A stream begins. An error leaves the device without one.
    fn configure(&mut self, width: u32, height: u32, cabac: bool) -> io::Result<Sizes>;

    /// One pass; the frame's bytes replace what `stream` holds. After an
    /// error other than a refused request the next pass must be a key
    /// frame of a new picture.
    fn encode(
        &mut self,
        pass: &Pass<'_>,
        units: &[RawUnit],
        stream: &mut Vec<u8>,
    ) -> io::Result<Made>;
}
