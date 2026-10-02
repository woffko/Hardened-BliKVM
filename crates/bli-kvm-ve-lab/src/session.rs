//! One stream on one engine: the buffers with their markers, the frames in
//! order, and the record of what was written.

use std::{io, time::Duration};

use crate::{
    engine::{Engine, Fault, Patience, Step, run},
    headers::{Slice, Stream, pps, slice_header, sps},
    picture::Picture,
    regs::{self, Adjust, Buffers, Hypothesis, Reconstruction, Search, Sizes},
};

/// Bytes kept before and after the part of a buffer the engine is given,
/// filled with a pattern that must survive every encode.
pub const GUARD: usize = 64 * 1024;
/// The stream buffer: more than any frame of the test sizes at any QP.
pub const STREAM_BYTES: usize = 4 * 1024 * 1024;

fn marker(position: usize) -> u8 {
    0xa5 ^ (position as u8).wrapping_mul(31)
}

#[derive(Debug)]
pub enum Error {
    Memory(io::Error),
    /// The engine could not be reset.
    Reset(io::Error),
    Fault(Fault),
    /// The encode ended with something other than "finished".
    Status(u32),
    /// The engine says it wrote more than the buffer holds, or no whole byte.
    Length(u32),
    /// A marker around a buffer changed: the engine wrote outside it.
    Marker(&'static str),
    /// The picture is not of the stream's size.
    Picture,
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Memory(error) => write!(formatter, "no buffer: {error}"),
            Self::Reset(error) => write!(formatter, "the engine was not reset: {error}"),
            Self::Fault(fault) => write!(formatter, "{fault}"),
            Self::Status(status) => write!(formatter, "the encode ended with status {status:#x}"),
            Self::Length(bits) => write!(formatter, "the engine reports {bits} bits"),
            Self::Marker(buffer) => {
                write!(formatter, "the engine wrote outside the {buffer} buffer")
            }
            Self::Picture => write!(formatter, "the picture is not of the stream's size"),
        }
    }
}

impl From<Fault> for Error {
    fn from(fault: Fault) -> Self {
        Self::Fault(fault)
    }
}

/// A buffer of the session: which of the engine's, and how much of it the
/// engine is given.
#[derive(Debug, Clone, Copy)]
struct Area {
    name: &'static str,
    buffer: usize,
    bytes: usize,
}

/// What the engine's bit writer made of the parameter sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderReport {
    /// The bytes a bit writer should have made.
    pub expected: Vec<u8>,
    /// The beginning of the stream buffer afterwards, 32 bytes more than
    /// were expected.
    pub found: Vec<u8>,
    /// The bits the engine says it has written.
    pub bits: u32,
    pub status: u32,
}

impl HeaderReport {
    pub fn matches(&self) -> bool {
        self.found.starts_with(&self.expected) && self.bits as usize == self.expected.len() * 8
    }
}

/// One encoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub key: bool,
    pub qp: i32,
    /// The frame as a decoder takes it: with SPS and PPS before a key frame.
    pub bytes: Vec<u8>,
    pub status: u32,
    /// Which of the session's sets of buffers holds its reconstruction.
    pub set: usize,
    /// From the start trigger to the end of the wait.
    pub time: Duration,
    /// The whole pass: the picture into the input buffer, the registers,
    /// the headers, the encode, the stream out of its buffer.
    pub total: Duration,
}

pub struct Session<E: Engine> {
    engine: E,
    hypothesis: Hypothesis,
    stream: Stream,
    areas: Vec<Area>,
    input: Area,
    output: Area,
    buffers: Buffers,
    /// Three sets of reconstruction buffers: the reference, and two for
    /// passes over the frame that is next, so that of two passes either
    /// can become the reference.
    sets: [Reconstruction; 3],
    /// The set that holds the reference.
    reference: usize,
    /// The set of a pass that the next pass must not write over.
    kept: Option<usize>,
    /// The search of the passes from here on; none is the hypothesis' own.
    search: Option<Search>,
    patience: Patience,
    index: u32,
    frame_num: u32,
    idr_pic_id: u32,
    /// The guards are checked after every so many passes (and by whoever
    /// asks `broken_guard`); 1 is every pass.
    guard_every: u32,
    passes: u32,
    /// What a guard holds while nothing wrote into it.
    guard: Vec<u8>,
    /// Fields of the frames' plan that this run tries otherwise.
    adjusts: Vec<Adjust>,
    /// Every step of every frame so far. A long run empties it as it goes.
    pub trace: Vec<Step>,
}

impl<E: Engine> Session<E> {
    /// Allocates buffers with room for a stream of `room`'s size (at least
    /// the stream's own), marks their guards and selects the encoder.
    pub fn open(
        mut engine: E,
        hypothesis: Hypothesis,
        stream: Stream,
        room: &Stream,
    ) -> Result<Self, Error> {
        let sizes = Sizes::lab(room);
        let needed = Sizes::lab(&stream);
        if needed.input > sizes.input || needed.reconstruction_luma > sizes.reconstruction_luma {
            return Err(Error::Picture);
        }
        let mut areas = Vec::new();
        let mut area = |engine: &mut E, name: &'static str, bytes: usize| -> Result<Area, Error> {
            let bytes = bytes.max(4096).div_ceil(4096) * 4096;
            let buffer = engine.allocate(bytes + 2 * GUARD).map_err(Error::Memory)?;
            let memory = engine.bytes(buffer);
            for (position, byte) in memory.iter_mut().enumerate() {
                *byte = if position < GUARD || position >= GUARD + bytes {
                    marker(position)
                } else {
                    0
                };
            }
            let area = Area {
                name,
                buffer,
                bytes,
            };
            areas.push(area);
            Ok(area)
        };
        let input = area(&mut engine, "input", sizes.input)?;
        let output = area(&mut engine, "stream", STREAM_BYTES)?;
        let pictures = sizes.reconstruction_luma + sizes.reconstruction_chroma;
        let first = area(&mut engine, "reconstruction 0", pictures)?;
        let second = area(&mut engine, "reconstruction 1", pictures)?;
        let third = area(&mut engine, "reconstruction 2", pictures)?;
        let first_subpixel = area(&mut engine, "sub-pixel 0", sizes.subpixel)?;
        let second_subpixel = area(&mut engine, "sub-pixel 1", sizes.subpixel)?;
        let third_subpixel = area(&mut engine, "sub-pixel 2", sizes.subpixel)?;
        let info = area(&mut engine, "macroblock info", sizes.macroblock_info)?;
        let extra = area(&mut engine, "extra", sizes.extra)?;
        // The address the engine is given: the buffer's, in the form the
        // hypothesis says.
        let at = |engine: &E, area: &Area| {
            (engine.address(area.buffer) + GUARD as u32).wrapping_sub(hypothesis.address_base)
        };
        let coded = stream.coded_width() * stream.coded_height();
        let reconstruction = |engine: &E, picture: &Area, subpixel: &Area| Reconstruction {
            luma: at(engine, picture),
            chroma: at(engine, picture) + sizes.reconstruction_luma as u32,
            subpixel: at(engine, subpixel),
        };
        let sets = [
            reconstruction(&engine, &first, &first_subpixel),
            reconstruction(&engine, &second, &second_subpixel),
            reconstruction(&engine, &third, &third_subpixel),
        ];
        let buffers = Buffers {
            input_luma: at(&engine, &input),
            input_chroma: at(&engine, &input) + coded,
            stream: at(&engine, &output),
            stream_bytes: output.bytes as u32,
            // A pass names its own two.
            reconstruction: [sets[1], sets[0]],
            macroblock_info: at(&engine, &info),
            extra: at(&engine, &extra),
        };
        let mut session = Self {
            engine,
            hypothesis,
            stream,
            areas,
            input,
            output,
            buffers,
            sets,
            reference: 0,
            kept: None,
            search: None,
            patience: Patience::default(),
            index: 0,
            frame_num: 0,
            idr_pic_id: 0,
            guard_every: 1,
            passes: 0,
            guard: (0..GUARD).map(marker).collect(),
            adjusts: Vec::new(),
            trace: Vec::new(),
        };
        let ops = regs::select(&session.hypothesis);
        run(
            &mut session.engine,
            &ops,
            session.patience,
            &mut session.trace,
        )?;
        Ok(session)
    }

    pub fn stream(&self) -> &Stream {
        &self.stream
    }

    pub fn buffers(&self) -> &Buffers {
        &self.buffers
    }

    pub fn engine(&mut self) -> &mut E {
        &mut self.engine
    }

    pub fn set_patience(&mut self, patience: Patience) {
        self.patience = patience;
    }

    /// Reading every guard takes longer than an encode; a run that measures
    /// time checks them less often.
    pub fn set_guard_checks(&mut self, every: u32) {
        self.guard_every = every.max(1);
    }

    /// The frames from here on are planned with these fields changed.
    pub fn set_adjusts(&mut self, adjusts: Vec<Adjust>) {
        self.adjusts = adjusts;
    }

    /// The passes from here on search like this; none is the hypothesis'
    /// own way.
    pub fn set_search(&mut self, search: Option<Search>) {
        self.search = search;
    }

    /// The next pass over the same frame leaves this pass's reconstruction
    /// alone, so that either of the two can be accepted.
    pub fn keep(&mut self, encoded: &Encoded) {
        self.kept = Some(encoded.set);
    }

    /// The frames accepted so far.
    pub fn frames(&self) -> u32 {
        self.index
    }

    /// The name of a buffer whose guards no longer hold their pattern.
    pub fn broken_guard(&mut self) -> Option<&'static str> {
        // The buffers' memory is not cached, and reading it byte by byte
        // takes longer than an encode: each guard is copied out in one run
        // and compared where it is cached. Every part of a buffer the
        // engine is given is a whole number of pages, so both guards begin
        // at the beginning of the pattern, which repeats every 256 bytes.
        let mut copy = vec![0u8; GUARD];
        for area in &self.areas {
            let memory = self.engine.bytes(area.buffer);
            for start in [0, GUARD + area.bytes] {
                copy.copy_from_slice(&memory[start..start + GUARD]);
                if copy != self.guard {
                    return Some(area.name);
                }
            }
        }
        None
    }

    /// Puts SPS and PPS through the engine's bit writer, `times` times over,
    /// and nothing else: the engine's first and smallest write to memory.
    /// Parameter sets may be repeated in a stream; more of them show in
    /// which portions the engine hands its bits to memory.
    pub fn put_parameter_sets(&mut self, times: usize) -> Result<HeaderReport, Error> {
        let mut header_ops = Vec::new();
        for _ in 0..times.max(1) {
            header_ops.extend(sps(&self.stream));
            header_ops.extend(pps(&self.stream));
        }
        let expected = crate::headers::bytes(&header_ops);
        let mut ops = regs::stream_buffer(&self.hypothesis, &self.buffers);
        ops.extend(regs::headers(&self.hypothesis, &header_ops));
        ops.extend(regs::headers_end(&self.hypothesis));
        run(&mut self.engine, &ops, self.patience, &mut self.trace)?;
        let status = self.engine.read(regs::AVC_STATUS);
        let bits = self.engine.read(regs::AVC_STREAM_LENGTH);
        if let Some(buffer) = self.broken_guard() {
            return Err(Error::Marker(buffer));
        }
        let found =
            self.engine.bytes(self.output.buffer)[GUARD..GUARD + expected.len() + 32].to_vec();
        Ok(HeaderReport {
            expected,
            found,
            bits,
            status,
        })
    }

    /// Encodes `picture` as the stream's next frame. The first frame is a
    /// key frame whatever `key` says.
    pub fn encode(&mut self, picture: &Picture, key: bool, qp: i32) -> Result<Encoded, Error> {
        let encoded = self.pass(picture, key, qp, true)?;
        self.accept(&encoded);
        Ok(encoded)
    }

    /// One pass over the frame that is next. The stream does not move: the
    /// same frame can be encoded again at another QP, against the same
    /// reference, until one pass is accepted. `fresh` says that the picture
    /// is not yet in the input buffer.
    pub fn pass(
        &mut self,
        picture: &Picture,
        key: bool,
        qp: i32,
        fresh: bool,
    ) -> Result<Encoded, Error> {
        let began = std::time::Instant::now();
        let (width, height) = (
            self.stream.coded_width() as usize,
            self.stream.coded_height() as usize,
        );
        if picture.coded_width != width || picture.coded_height != height {
            return Err(Error::Picture);
        }
        let key = key || self.index == 0;
        let slice = Slice {
            key,
            frame_num: if key { 0 } else { self.frame_num },
            idr_pic_id: self.idr_pic_id,
            qp,
            cabac_init_idc: 0,
        };
        if fresh {
            let memory = self.engine.bytes(self.input.buffer);
            memory[GUARD..GUARD + picture.luma.len()].copy_from_slice(&picture.luma);
            memory[GUARD + picture.luma.len()..GUARD + picture.bytes()]
                .copy_from_slice(&picture.chroma);
        }

        let mut header_ops = Vec::new();
        if key {
            header_ops.extend(sps(&self.stream));
            header_ops.extend(pps(&self.stream));
        }
        header_ops.extend(slice_header(&self.stream, &slice));
        let stride = width as u32;
        if self.hypothesis.reset_each_frame {
            self.engine.reset().map_err(Error::Reset)?;
            let ops = regs::select(&self.hypothesis);
            run(&mut self.engine, &ops, self.patience, &mut self.trace)?;
        }
        // Written where neither the reference nor a kept pass is. The
        // plan takes its own set from the first place and the reference
        // from the second.
        let own = (0..self.sets.len())
            .find(|set| *set != self.reference && Some(*set) != self.kept)
            .expect("three sets, two of them in use");
        let buffers = Buffers {
            reconstruction: [self.sets[own], self.sets[self.reference]],
            ..self.buffers
        };
        let mut ops = regs::adjusted(
            regs::frame(
                &self.hypothesis,
                &self.stream,
                stride,
                &slice,
                0,
                &buffers,
                &header_ops,
            ),
            &self.adjusts,
        );
        if let Some(search) = self.search {
            ops = regs::adjusted(ops, &[search.adjust()]);
        }
        let outcome = run(&mut self.engine, &ops, self.patience, &mut self.trace)?;

        let status = self.engine.read(regs::AVC_STATUS) & regs::STATUS_ENDED;
        self.engine.write(regs::AVC_STATUS, regs::STATUS_ENDED);
        self.engine.write(regs::AVC_INT_EN, 0);
        let bits = self.engine.read(regs::AVC_STREAM_LENGTH);
        self.passes += 1;
        if self.passes % self.guard_every == 0 {
            if let Some(buffer) = self.broken_guard() {
                return Err(Error::Marker(buffer));
            }
        }
        if status != regs::STATUS_FINISH {
            return Err(Error::Status(status));
        }
        let length = (bits / 8) as usize;
        if bits % 8 != 0 || length == 0 || length > self.output.bytes {
            return Err(Error::Length(bits));
        }
        let bytes = self.engine.bytes(self.output.buffer)[GUARD..GUARD + length].to_vec();
        Ok(Encoded {
            key,
            qp,
            bytes,
            status,
            set: own,
            time: outcome.encode_time.unwrap_or_default(),
            total: began.elapsed(),
        })
    }

    /// A frame that says "nothing changed" in the place of the frame that
    /// is next: made in software, the engine is not asked. The stream goes
    /// on by one frame; the reference stays the last frame the engine made,
    /// which is what a decoder shows for this one.
    pub fn unchanged(&mut self) -> Encoded {
        let began = std::time::Instant::now();
        let bytes = crate::standin::unchanged_frame(&self.stream, self.frame_num);
        self.frame_num += 1;
        self.kept = None;
        Encoded {
            key: false,
            qp: crate::headers::INITIAL_QP,
            bytes,
            status: 0,
            set: self.reference,
            time: Duration::ZERO,
            total: began.elapsed(),
        }
    }

    /// The pass is the frame: the next one refers to it.
    pub fn accept(&mut self, encoded: &Encoded) {
        self.reference = encoded.set;
        self.kept = None;
        self.index += 1;
        self.frame_num = if encoded.key { 1 } else { self.frame_num + 1 };
        if encoded.key {
            self.idr_pic_id ^= 1;
        }
    }

    /// Gives the engine back: no encoder selected.
    pub fn close(mut self) -> Result<E, Error> {
        let ops = regs::deselect(&self.hypothesis);
        run(&mut self.engine, &ops, self.patience, &mut self.trace)?;
        Ok(self.engine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bits::{BitSource, without_prevention},
        engine::FakeEngine,
        picture::{Sequence, render},
        regs::Search,
    };

    fn stream(width: u32, height: u32) -> Stream {
        Stream {
            width,
            height,
            fps: 30,
            cabac: false,
            chroma_qp_offset: 4,
            deblocking: false,
        }
    }

    /// The units of an Annex B stream: header byte and payload.
    fn units(bytes: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut starts = Vec::new();
        let mut position = 0;
        while position + 4 <= bytes.len() {
            if bytes[position..position + 4] == [0, 0, 0, 1] {
                starts.push(position);
                position += 4;
            } else {
                position += 1;
            }
        }
        starts
            .iter()
            .enumerate()
            .map(|(index, start)| {
                let end = starts.get(index + 1).copied().unwrap_or(bytes.len());
                (bytes[start + 4], bytes[start + 5..end].to_vec())
            })
            .collect()
    }

    #[test]
    fn both_maps_make_the_same_stream_on_the_stand_in() {
        let stream = stream(320, 240);
        let mut picture = Picture::new(320, 240);
        let mut streams = Vec::new();
        for hypothesis in [Hypothesis::OLDER, Hypothesis::V3S] {
            let mut session =
                Session::open(FakeEngine::new(), hypothesis, stream, &stream).unwrap();
            let mut frames = Vec::new();
            for frame in 0..4 {
                render(Sequence::Drag, frame, &mut picture);
                // The fourth frame is asked to be a key frame.
                frames.push(session.encode(&picture, frame == 3, 30).unwrap());
            }
            assert!(session.broken_guard().is_none());
            let keys: Vec<bool> = frames.iter().map(|frame| frame.key).collect();
            assert_eq!(keys, [true, false, false, true]);
            session.close().unwrap();
            let bytes: Vec<Vec<u8>> = frames.into_iter().map(|frame| frame.bytes).collect();
            streams.push(bytes);
        }
        assert_eq!(streams[0], streams[1]);

        // A key frame: SPS, PPS, an IDR slice of I_PCM macroblocks with the
        // picture's own samples.
        let key = units(&streams[0][0]);
        let headers: Vec<u8> = key.iter().map(|unit| unit.0).collect();
        assert_eq!(headers, [0x67, 0x68, 0x65]);
        let payload = without_prevention(&key[2].1);
        let mut source = BitSource::new(&payload);
        assert_eq!(source.ue(), Some(0));
        assert_eq!(source.ue(), Some(7));
        assert_eq!(source.ue(), Some(0));
        assert_eq!(source.bits(8), Some(0));
        assert_eq!(source.ue(), Some(0));
        assert_eq!(source.bits(2), Some(0));
        assert_eq!(source.se(), Some(4));
        assert_eq!(source.ue(), Some(1));
        assert_eq!(source.ue(), Some(25));
        source.align();
        render(Sequence::Drag, 0, &mut picture);
        let at = source.position() / 8;
        assert_eq!(payload[at..at + 16], picture.luma[..16]);

        // A P frame: one unit of a few bytes; its frame number counts on.
        let later = units(&streams[0][2]);
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].0, 0x41);
        let mut source = BitSource::new(&later[0].1);
        assert_eq!(source.ue(), Some(0));
        assert_eq!(source.ue(), Some(5));
        assert_eq!(source.ue(), Some(0));
        assert_eq!(source.bits(8), Some(2));
        // The second key frame has the other IDR id.
        let again = units(&streams[0][3]);
        let payload = without_prevention(&again[2].1);
        let mut source = BitSource::new(&payload);
        source.ue();
        source.ue();
        source.ue();
        assert_eq!(source.bits(8), Some(0));
        assert_eq!(source.ue(), Some(1));
    }

    #[test]
    fn the_bit_writer_alone_writes_the_parameter_sets() {
        let stream = stream(1920, 1080);
        for hypothesis in [Hypothesis::OLDER, Hypothesis::V3S] {
            let mut session =
                Session::open(FakeEngine::new(), hypothesis, stream, &stream).unwrap();
            let report = session.put_parameter_sets(1).unwrap();
            assert!(report.matches(), "{report:?}");
            assert_eq!(report.expected[..5], [0, 0, 0, 1, 0x67]);
            let many = session.put_parameter_sets(40).unwrap();
            assert!(many.matches());
            assert_eq!(many.expected.len(), 40 * report.expected.len());
            assert_eq!(report.found[report.expected.len()..], [0; 32]);
            // Nothing was encoded.
            assert_eq!(report.status & regs::STATUS_ENDED, 0);
            assert!(
                !session
                    .engine()
                    .writes
                    .contains(&(regs::AVC_TRIGGER, regs::TRIGGER_START))
            );
        }
    }

    #[test]
    fn addresses_are_given_in_the_form_the_hypothesis_says() {
        let stream = stream(320, 240);
        let mut picture = Picture::new(320, 240);
        render(Sequence::Drag, 0, &mut picture);
        // An engine that counts from the memory's beginning works with the
        // hypothesis that says so, and with no other.
        let engine = FakeEngine::counting_from(regs::MEMORY_BASE);
        let mut session = Session::open(engine, Hypothesis::V3S_RELATIVE, stream, &stream).unwrap();
        assert!(session.buffers().stream < regs::MEMORY_BASE);
        assert!(session.put_parameter_sets(1).unwrap().matches());
        assert!(session.encode(&picture, true, 30).is_ok());
        let engine = FakeEngine::counting_from(regs::MEMORY_BASE);
        let mut session = Session::open(engine, Hypothesis::V3S, stream, &stream).unwrap();
        assert!(!session.put_parameter_sets(1).unwrap().matches());
        assert!(matches!(
            session.encode(&picture, true, 30),
            Err(Error::Status(regs::STATUS_STALL))
        ));
    }

    #[test]
    fn the_newer_generation_works_with_its_hypothesis_and_no_other() {
        let stream = Stream {
            chroma_qp_offset: 0,
            ..stream(320, 240)
        };
        let mut picture = Picture::new(320, 240);
        let mut session =
            Session::open(FakeEngine::newer(), Hypothesis::H6, stream, &stream).unwrap();
        // The encoder block is told units of 256 bytes, the input stage not.
        let buffers = *session.buffers();
        assert!(
            session
                .engine()
                .writes
                .iter()
                .all(|(reg, _)| *reg != regs::AVC_STREAM_START)
        );
        let report = session.put_parameter_sets(3).unwrap();
        assert!(report.matches(), "{report:?}");
        assert!(
            session
                .engine()
                .writes
                .contains(&(regs::AVC_STREAM_START, buffers.stream >> 8))
        );
        assert_eq!(
            session.engine().writes.last(),
            Some(&(regs::AVC_TRIGGER, regs::TRIGGER_ALIGN))
        );
        let mut frames = Vec::new();
        for frame in 0..3 {
            render(Sequence::Drag, frame, &mut picture);
            frames.push(session.encode(&picture, false, 30).unwrap());
        }
        assert!(session.broken_guard().is_none());
        assert!(
            session
                .engine()
                .writes
                .contains(&(regs::ISP_INPUT_LUMA, buffers.input_luma))
        );
        let key = units(&frames[0].bytes);
        let headers: Vec<u8> = key.iter().map(|unit| unit.0).collect();
        assert_eq!(headers, [0x67, 0x68, 0x65]);
        assert_eq!(units(&frames[2].bytes)[0].0, 0x41);
        session.close().unwrap();

        // The map's form on this engine: the stream register names no
        // buffer, as on the board in V2.1.
        let mut session =
            Session::open(FakeEngine::newer(), Hypothesis::V3S, stream, &stream).unwrap();
        assert!(!session.put_parameter_sets(1).unwrap().matches());
        render(Sequence::Drag, 0, &mut picture);
        assert!(matches!(
            session.encode(&picture, true, 30),
            Err(Error::Status(regs::STATUS_STALL))
        ));
        // And this engine's form on the maps' engine.
        let mut session =
            Session::open(FakeEngine::new(), Hypothesis::H6, stream, &stream).unwrap();
        assert!(!session.put_parameter_sets(1).unwrap().matches());
    }

    #[test]
    fn a_frame_can_be_encoded_again_before_it_is_accepted() {
        let stream = stream(320, 240);
        let mut picture = Picture::new(320, 240);
        let mut twice = Session::open(FakeEngine::new(), Hypothesis::V3S, stream, &stream).unwrap();
        let mut once = Session::open(FakeEngine::new(), Hypothesis::V3S, stream, &stream).unwrap();
        for frame in 0..4 {
            render(Sequence::Drag, frame, &mut picture);
            // A first pass at a QP that is thrown away, then the one that
            // counts: the stream is the one of a session that never tried.
            let first = twice.pass(&picture, false, 20, true).unwrap();
            assert_eq!(twice.frames(), frame);
            let second = twice.pass(&picture, false, 30, false).unwrap();
            twice.accept(&second);
            assert_eq!(twice.frames(), frame + 1);
            let plain = once.encode(&picture, false, 30).unwrap();
            assert_eq!(second.bytes, plain.bytes);
            assert_eq!(second.key, frame == 0);
            assert_eq!((first.qp, second.qp), (20, 30));
            assert_ne!(first.bytes, second.bytes);
        }
        // Guards are still checked when asked for less often.
        twice.set_guard_checks(100);
        render(Sequence::Drag, 4, &mut picture);
        assert!(twice.encode(&picture, false, 30).is_ok());
        assert_eq!(twice.broken_guard(), None);
    }

    #[test]
    fn of_two_passes_either_can_become_the_reference() {
        // The last value written to a register.
        fn last(session: &mut Session<FakeEngine>, reg: u32) -> u32 {
            let writes = &session.engine().writes;
            writes.iter().rev().find(|write| write.0 == reg).unwrap().1
        }
        let stream = stream(320, 240);
        let mut picture = Picture::new(320, 240);
        let mut session =
            Session::open(FakeEngine::newer(), Hypothesis::H6, stream, &stream).unwrap();
        render(Sequence::Drag, 0, &mut picture);
        let key = session.encode(&picture, true, 30).unwrap();
        assert_eq!(last(&mut session, regs::AVC_ME), regs::ME_NEWER);

        let mut reference = last(&mut session, regs::AVC_REC_LUMA);
        for (frame, take_first) in [(1, true), (2, false), (3, true)] {
            render(Sequence::Drag, frame, &mut picture);
            session.set_search(Some(Search::Exact));
            let first = session.pass(&picture, false, 30, true).unwrap();
            assert_eq!(last(&mut session, regs::AVC_ME), regs::ME_NEWER);
            assert_eq!(last(&mut session, regs::AVC_REF_LUMA), reference);
            let first_written = last(&mut session, regs::AVC_REC_LUMA);
            session.keep(&first);
            session.set_search(Some(Search::Wide));
            let second = session.pass(&picture, false, 30, false).unwrap();
            assert_eq!(
                last(&mut session, regs::AVC_ME),
                regs::ME_NEWER | regs::ME_REFINE_OFF
            );
            // The same reference, and a set of its own.
            assert_eq!(last(&mut session, regs::AVC_REF_LUMA), reference);
            let second_written = last(&mut session, regs::AVC_REC_LUMA);
            assert_ne!(first.set, second.set);
            assert_ne!(first_written, second_written);
            assert_ne!(first_written, reference);
            assert_ne!(second_written, reference);

            let (taken, written) = if take_first {
                (&first, first_written)
            } else {
                (&second, second_written)
            };
            session.accept(taken);
            // What the next pass refers to is what the accepted one wrote.
            render(Sequence::Drag, frame + 1, &mut picture);
            session.set_search(None);
            let next = session.pass(&picture, false, 30, true).unwrap();
            assert_eq!(last(&mut session, regs::AVC_REF_LUMA), written);
            assert_eq!(last(&mut session, regs::AVC_ME), regs::ME_NEWER);
            assert_ne!(next.set, taken.set);
            // That pass is thrown away: the stream goes on from `taken`.
            reference = written;
        }
        assert_eq!(session.frames(), 4);
        assert_ne!(key.set, 0);
        assert_eq!(session.broken_guard(), None);
    }

    #[test]
    fn a_frame_left_out_counts_in_the_stream_and_not_for_the_engine() {
        let stream = Stream {
            chroma_qp_offset: 0,
            ..stream(320, 240)
        };
        let mut picture = Picture::new(320, 240);
        let mut session =
            Session::open(FakeEngine::newer(), Hypothesis::H6, stream, &stream).unwrap();
        render(Sequence::Drag, 0, &mut picture);
        session.encode(&picture, false, 30).unwrap();
        // The second frame is tried and left out; the third is encoded.
        render(Sequence::Drag, 1, &mut picture);
        session.pass(&picture, false, 51, true).unwrap();
        let unchanged = session.unchanged();
        assert_eq!(session.frames(), 1);
        render(Sequence::Drag, 2, &mut picture);
        let third = session.encode(&picture, false, 30).unwrap();
        assert_eq!(session.frames(), 2);
        // In the stream the frames count on: 1 for the one left out, 2 after it.
        let frame_num = |bytes: &[u8]| {
            let unit = &units(bytes)[0];
            assert_eq!(unit.0, 0x41);
            let mut source = BitSource::new(&unit.1);
            source.ue();
            source.ue();
            source.ue();
            source.bits(8)
        };
        assert_eq!(frame_num(&unchanged.bytes), Some(1));
        assert_eq!(frame_num(&third.bytes), Some(2));
        assert!(unchanged.bytes.len() < 16);
    }

    #[test]
    fn a_small_picture_fits_buffers_made_for_a_large_one() {
        let small = stream(320, 240);
        let room = stream(1920, 1080);
        let mut session = Session::open(FakeEngine::new(), Hypothesis::V3S, small, &room).unwrap();
        let mut picture = Picture::new(320, 240);
        render(Sequence::Scroll, 2, &mut picture);
        let frame = session.encode(&picture, true, 26).unwrap();
        // 300 macroblocks of 384 samples, and more for the headers.
        assert!(frame.bytes.len() > 300 * 384);
        // The other way round there is no room.
        assert!(matches!(
            Session::open(FakeEngine::new(), Hypothesis::V3S, room, &small),
            Err(Error::Picture)
        ));
    }

    #[test]
    fn a_write_outside_a_buffer_is_noticed() {
        let stream = stream(320, 240);
        let mut session =
            Session::open(FakeEngine::new(), Hypothesis::OLDER, stream, &stream).unwrap();
        assert_eq!(session.broken_guard(), None);
        // What an engine with a wrong stride would do: one byte past the end
        // of the first reconstruction buffer.
        let buffer = session.areas[2];
        session.engine.bytes(buffer.buffer)[GUARD + buffer.bytes] ^= 1;
        assert_eq!(session.broken_guard(), Some("reconstruction 0"));
        let mut picture = Picture::new(320, 240);
        render(Sequence::Pointer, 0, &mut picture);
        assert!(matches!(
            session.encode(&picture, true, 30),
            Err(Error::Marker("reconstruction 0"))
        ));
    }

    #[test]
    fn a_picture_of_another_size_is_refused() {
        let stream = stream(320, 240);
        let mut session =
            Session::open(FakeEngine::new(), Hypothesis::V3S, stream, &stream).unwrap();
        let picture = Picture::new(640, 480);
        assert!(matches!(
            session.encode(&picture, true, 30),
            Err(Error::Picture)
        ));
    }
}
