//! The encoder driver's device with the lab tool as its client: the same
//! pictures and the same headers as through the lab module, but every
//! register is the driver's. What the two ways make of a sequence is
//! compared byte for byte (gate V4). The requests themselves are the
//! product's (`bli_kvm_ve_encoder::driver`).

use std::{io, path::Path, time::Instant};

use bli_kvm_ve_encoder::{
    device::{Device, Pass, Picture as Given, Reference},
    driver::Driver,
};

use crate::{
    headers::{self, Slice, Stream},
    picture::Picture,
    regs::Search,
    session::Encoded,
};

pub use bli_kvm_ve_encoder::abi::DEVICE;

/// One stream through the driver.
pub struct DriverStream {
    device: Driver,
    stream: Stream,
    picture_bytes: usize,
    /// The picture as the driver takes it: luma, then chroma.
    picture: Vec<u8>,
    output: Vec<u8>,
    frames: u32,
    frame_num: u32,
    idr_pic_id: u32,
    /// The search of the passes from here on; none is the driver's own.
    search: Option<Search>,
    /// Passes over the picture that is next, so far.
    passes: usize,
    /// What the next new picture refers to.
    reference: Reference,
    /// The engine's identifications as the driver read them.
    pub decoder_ip: u32,
    pub encoder_ip: u32,
    /// The engine's three numbers about the last frame.
    pub statistics: [u32; 3],
}

impl DriverStream {
    pub fn open(stream: Stream) -> io::Result<Self> {
        let mut device = Driver::open(Path::new(DEVICE))?;
        let sizes = device.configure(stream.width, stream.height, stream.cabac)?;
        let expected = (stream.coded_width() * stream.coded_height() * 3 / 2) as usize;
        if sizes.picture_bytes != expected || sizes.stream_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the driver's picture is not the lab's",
            ));
        }
        Ok(Self {
            decoder_ip: device.decoder_ip,
            encoder_ip: device.encoder_ip,
            device,
            stream,
            picture_bytes: expected,
            picture: vec![0; expected],
            output: Vec::new(),
            frames: 0,
            frame_num: 0,
            idr_pic_id: 0,
            search: None,
            passes: 0,
            reference: Reference::Last,
            statistics: [0; 3],
        })
    }

    pub fn frames(&self) -> u32 {
        self.frames
    }

    /// The passes from here on search like this; none is the driver's own
    /// way, the exact one.
    pub fn set_search(&mut self, search: Option<Search>) {
        self.search = search;
    }

    /// One pass over the frame that is next, as `Session::pass`: with
    /// `fresh` the picture is sent, without it the driver encodes the one it
    /// has again.
    pub fn pass(
        &mut self,
        picture: &Picture,
        key: bool,
        qp: i32,
        fresh: bool,
    ) -> Result<Encoded, String> {
        let began = Instant::now();
        if picture.bytes() != self.picture_bytes {
            return Err("the picture is not of the stream's size".into());
        }
        // A first pass over a picture says which pass over the picture
        // before it is the frame; a later one is the picture again.
        if (self.passes > 0) == fresh {
            return Err("a picture is new with its first pass and with no other".into());
        }
        let key = key || self.frames == 0;
        let slice = Slice {
            key,
            frame_num: if key { 0 } else { self.frame_num },
            idr_pic_id: self.idr_pic_id,
            qp,
            cabac_init_idc: 0,
        };
        let mut header_ops = Vec::new();
        if key {
            header_ops.extend(headers::sps(&self.stream));
            header_ops.extend(headers::pps(&self.stream));
        }
        header_ops.extend(headers::slice_header(&self.stream, &slice));
        let units = headers::units(&header_ops);
        if fresh {
            self.picture[..picture.luma.len()].copy_from_slice(&picture.luma);
            self.picture[picture.luma.len()..].copy_from_slice(&picture.chroma);
        }
        let pass = Pass {
            key,
            qp: qp as u32,
            search: self.search.unwrap_or(Search::Exact),
            picture: if fresh {
                Given::New {
                    nv12: &self.picture,
                    reference: self.reference,
                }
            } else {
                Given::Again
            },
        };
        let made = self
            .device
            .encode(&pass, &units, &mut self.output)
            .map_err(|error| format!("the driver refused the frame: {error}"))?;
        self.statistics = made.statistics;
        self.passes += 1;
        Ok(Encoded {
            key,
            qp,
            bytes: self.output.clone(),
            status: made.status,
            // For the driver a pass is known by its place among the
            // picture's passes.
            set: self.passes - 1,
            time: made.time,
            total: began.elapsed(),
        })
    }

    /// As `Session::keep`. The driver never writes over the last pass, and
    /// a pass after it is the frame or the picture is left out: nothing
    /// needs saying.
    pub fn keep(&mut self, _encoded: &Encoded) {}

    /// A frame that says "nothing changed" in the place of the frame that
    /// is next, as `Session::unchanged`: the driver is not asked, and is
    /// told with the next picture that the stream stayed where it was.
    pub fn unchanged(&mut self) -> Encoded {
        let began = Instant::now();
        let bytes = crate::standin::unchanged_frame(&self.stream, self.frame_num);
        self.frame_num += 1;
        if self.passes > 0 {
            self.reference = Reference::Stay;
        }
        self.passes = 0;
        Encoded {
            key: false,
            qp: headers::INITIAL_QP,
            bytes,
            status: 0,
            set: 0,
            time: std::time::Duration::ZERO,
            total: began.elapsed(),
        }
    }

    /// The pass is the frame: the next picture refers to it, and its
    /// headers count on from it. It is the last pass over its picture or
    /// the one before it.
    pub fn accept(&mut self, encoded: &Encoded) {
        self.reference = if encoded.set + 1 == self.passes {
            Reference::Last
        } else {
            assert_eq!(
                encoded.set + 2,
                self.passes,
                "the driver has the last two passes"
            );
            Reference::Earlier
        };
        self.passes = 0;
        self.frames += 1;
        self.frame_num = if encoded.key { 1 } else { self.frame_num + 1 };
        if encoded.key {
            self.idr_pic_id ^= 1;
        }
    }
}
