//! The headers of the stream: one SPS, one PPS and a slice header per frame,
//! as lists of writes to a bit writer. The engine's bit writer and the
//! stand-in's take the same lists.
//!
//! The syntax is that of the product's x264 stream: constrained baseline,
//! frames only, one reference frame, one slice per picture, picture order
//! type 2 (output order is decoding order).

use crate::bits::Put;

/// What the stream is, from its first frame to its last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stream {
    /// The picture's visible size.
    pub width: u32,
    pub height: u32,
    /// Frames a second, for the level only.
    pub fps: u32,
    /// CABAC is not part of the baseline profiles; the stream then says
    /// main profile.
    pub cabac: bool,
    /// `chroma_qp_index_offset` of the PPS; the engine is told the same.
    pub chroma_qp_offset: i32,
    /// Whether slices leave the deblocking filter on.
    pub deblocking: bool,
}

/// `pic_init_qp` of the PPS. A frame's QP is given in its slice header
/// relative to it, so the PPS does not change with the QP.
pub const INITIAL_QP: i32 = 26;
/// The bits of `frame_num`.
pub const FRAME_NUM_BITS: u8 = 8;

impl Stream {
    pub fn width_mbs(&self) -> u32 {
        self.width.div_ceil(16)
    }

    pub fn height_mbs(&self) -> u32 {
        self.height.div_ceil(16)
    }

    /// The coded size: whole macroblocks.
    pub fn coded_width(&self) -> u32 {
        self.width_mbs() * 16
    }

    pub fn coded_height(&self) -> u32 {
        self.height_mbs() * 16
    }

    pub fn macroblocks(&self) -> u32 {
        self.width_mbs() * self.height_mbs()
    }

    /// The lowest level, from 3.0 up, whose frame size and macroblock rate
    /// hold this stream (table A-1 of the standard).
    pub fn level_idc(&self) -> u8 {
        const LEVELS: [(u8, u32, u32); 7] = [
            (30, 40_500, 1_620),
            (31, 108_000, 3_600),
            (32, 216_000, 5_120),
            (40, 245_760, 8_192),
            (42, 522_240, 8_704),
            (50, 589_824, 22_080),
            (51, 983_040, 36_864),
        ];
        let size = self.macroblocks();
        let rate = size * self.fps;
        LEVELS
            .iter()
            .find(|(_, max_rate, max_size)| size <= *max_size && rate <= *max_rate)
            .map_or(52, |(level, _, _)| *level)
    }

    /// 66 with the first two constraint flags is constrained baseline; with
    /// CABAC the stream is main profile.
    pub fn profile_idc(&self) -> u8 {
        if self.cabac { 77 } else { 66 }
    }

    pub fn constraint_flags(&self) -> u8 {
        if self.cabac { 0x40 } else { 0xc0 }
    }
}

/// One frame's part of the headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slice {
    /// An IDR frame; otherwise a P frame.
    pub key: bool,
    /// Counts the frames since the last key frame.
    pub frame_num: u32,
    /// Differs between two key frames that follow each other.
    pub idr_pic_id: u32,
    pub qp: i32,
    /// Which of the three CABAC start tables a P slice uses.
    pub cabac_init_idc: u32,
}

/// A step of writing headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderOp {
    /// A start code and the unit's header byte, written without emulation
    /// prevention.
    Start(u8),
    /// Bits of the unit's payload, written with emulation prevention.
    Bits(Put),
}

const NAL_SLICE: u8 = 1;
const NAL_IDR_SLICE: u8 = 5;
const NAL_SPS: u8 = 7;
const NAL_PPS: u8 = 8;

/// Collects one unit and counts its bits, so that it can end on a byte.
struct Unit {
    ops: Vec<HeaderOp>,
    bits: usize,
}

impl Unit {
    fn new(ref_idc: u8, unit_type: u8) -> Self {
        Self {
            ops: vec![HeaderOp::Start((ref_idc << 5) | unit_type)],
            bits: 0,
        }
    }

    fn put(&mut self, put: Put) {
        self.bits += put.bits as usize;
        self.ops.push(HeaderOp::Bits(put));
    }

    fn u(&mut self, bits: u8, value: u32) {
        self.put(Put::new(value, bits));
    }

    fn flag(&mut self, value: bool) {
        self.u(1, value as u32);
    }

    fn ue(&mut self, value: u32) {
        self.put(Put::ue(value));
    }

    fn se(&mut self, value: i32) {
        self.put(Put::se(value));
    }

    /// The stop bit and zero bits up to the byte's end.
    fn trailing(&mut self) {
        let zeros = (8 - (self.bits + 1) % 8) % 8;
        self.u(zeros as u8 + 1, 1 << zeros);
    }
}

pub fn sps(stream: &Stream) -> Vec<HeaderOp> {
    let mut unit = Unit::new(3, NAL_SPS);
    unit.u(8, stream.profile_idc() as u32);
    unit.u(8, stream.constraint_flags() as u32);
    unit.u(8, stream.level_idc() as u32);
    unit.ue(0); // seq_parameter_set_id
    unit.ue(FRAME_NUM_BITS as u32 - 4); // log2_max_frame_num_minus4
    unit.ue(2); // pic_order_cnt_type
    unit.ue(1); // max_num_ref_frames
    unit.flag(false); // gaps_in_frame_num_value_allowed_flag
    unit.ue(stream.width_mbs() - 1);
    unit.ue(stream.height_mbs() - 1);
    unit.flag(true); // frame_mbs_only_flag
    unit.flag(true); // direct_8x8_inference_flag
    let crop_right = (stream.coded_width() - stream.width) / 2;
    let crop_bottom = (stream.coded_height() - stream.height) / 2;
    let cropped = crop_right != 0 || crop_bottom != 0;
    unit.flag(cropped);
    if cropped {
        unit.ue(0);
        unit.ue(crop_right);
        unit.ue(0);
        unit.ue(crop_bottom);
    }
    // VUI, for one thing only: frames leave the decoder in the order they
    // come, at once.
    unit.flag(true); // vui_parameters_present_flag
    unit.flag(false); // aspect_ratio_info_present_flag
    unit.flag(false); // overscan_info_present_flag
    unit.flag(false); // video_signal_type_present_flag
    unit.flag(false); // chroma_loc_info_present_flag
    unit.flag(false); // timing_info_present_flag
    unit.flag(false); // nal_hrd_parameters_present_flag
    unit.flag(false); // vcl_hrd_parameters_present_flag
    unit.flag(false); // pic_struct_present_flag
    unit.flag(true); // bitstream_restriction_flag
    unit.flag(true); // motion_vectors_over_pic_boundaries_flag
    unit.ue(0); // max_bytes_per_pic_denom
    unit.ue(0); // max_bits_per_mb_denom
    unit.ue(16); // log2_max_mv_length_horizontal
    unit.ue(16); // log2_max_mv_length_vertical
    unit.ue(0); // max_num_reorder_frames
    unit.ue(1); // max_dec_frame_buffering
    unit.trailing();
    unit.ops
}

pub fn pps(stream: &Stream) -> Vec<HeaderOp> {
    let mut unit = Unit::new(3, NAL_PPS);
    unit.ue(0); // pic_parameter_set_id
    unit.ue(0); // seq_parameter_set_id
    unit.flag(stream.cabac); // entropy_coding_mode_flag
    unit.flag(false); // bottom_field_pic_order_in_frame_present_flag
    unit.ue(0); // num_slice_groups_minus1
    unit.ue(0); // num_ref_idx_l0_default_active_minus1
    unit.ue(0); // num_ref_idx_l1_default_active_minus1
    unit.flag(false); // weighted_pred_flag
    unit.u(2, 0); // weighted_bipred_idc
    unit.se(INITIAL_QP - 26); // pic_init_qp_minus26
    unit.se(INITIAL_QP - 26); // pic_init_qs_minus26
    unit.se(stream.chroma_qp_offset);
    unit.flag(true); // deblocking_filter_control_present_flag
    unit.flag(false); // constrained_intra_pred_flag
    unit.flag(false); // redundant_pic_cnt_present_flag
    unit.trailing();
    unit.ops
}

/// The slice header. It ends where the slice's data begins, which need not
/// be a byte's end: the data follows in the same unit.
pub fn slice_header(stream: &Stream, slice: &Slice) -> Vec<HeaderOp> {
    let mut unit = if slice.key {
        Unit::new(3, NAL_IDR_SLICE)
    } else {
        Unit::new(2, NAL_SLICE)
    };
    unit.ue(0); // first_mb_in_slice
    // 7 and 5: an I or P slice in a picture whose slices are all of its type.
    unit.ue(if slice.key { 7 } else { 5 });
    unit.ue(0); // pic_parameter_set_id
    unit.u(FRAME_NUM_BITS, slice.frame_num % (1 << FRAME_NUM_BITS));
    if slice.key {
        unit.ue(slice.idr_pic_id);
    } else {
        unit.flag(false); // num_ref_idx_active_override_flag
        unit.flag(false); // ref_pic_list_modification_flag_l0
    }
    // Reference picture marking: every frame is a reference.
    if slice.key {
        unit.flag(false); // no_output_of_prior_pics_flag
        unit.flag(false); // long_term_reference_flag
    } else {
        unit.flag(false); // adaptive_ref_pic_marking_mode_flag
    }
    if stream.cabac && !slice.key {
        unit.ue(slice.cabac_init_idc);
    }
    unit.se(slice.qp - INITIAL_QP); // slice_qp_delta
    if stream.deblocking {
        unit.ue(0); // disable_deblocking_filter_idc
        unit.se(0); // slice_alpha_c0_offset_div2
        unit.se(0); // slice_beta_offset_div2
    } else {
        unit.ue(1);
    }
    unit.ops
}

/// What a bit writer makes of header steps: the bytes of the stream.
pub fn bytes(ops: &[HeaderOp]) -> Vec<u8> {
    let mut sink = crate::bits::BitSink::new();
    for op in ops {
        match *op {
            HeaderOp::Start(byte) => {
                sink.set_prevention(false);
                sink.put(Put::new(0, 16));
                sink.put(Put::new(1, 16));
                sink.put(Put::new(byte as u32, 8));
                sink.set_prevention(true);
            }
            HeaderOp::Bits(put) => sink.put(put),
        }
    }
    sink.align();
    sink.into_bytes()
}

/// One unit as the encoder driver is given it: the byte after the start
/// code, and the payload's bits, the first byte's most significant first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawUnit {
    pub header: u8,
    pub payload: Vec<u8>,
    pub bits: usize,
}

impl RawUnit {
    /// The unit as writes to a bit writer, sixteen bits at a time: what the
    /// driver makes of it.
    pub fn ops(&self) -> Vec<HeaderOp> {
        let mut ops = vec![HeaderOp::Start(self.header)];
        let mut done = 0;
        while done < self.bits {
            let take = (self.bits - done).min(16);
            let value = (done..done + take).fold(0u32, |value, bit| {
                (value << 1) | ((self.payload[bit / 8] >> (7 - bit % 8)) & 1) as u32
            });
            ops.push(HeaderOp::Bits(Put::new(value, take as u8)));
            done += take;
        }
        ops
    }
}

/// The units of a list of header writes.
pub fn units(ops: &[HeaderOp]) -> Vec<RawUnit> {
    let mut units: Vec<RawUnit> = Vec::new();
    for op in ops {
        match *op {
            HeaderOp::Start(header) => units.push(RawUnit {
                header,
                payload: Vec::new(),
                bits: 0,
            }),
            HeaderOp::Bits(put) => {
                let unit = units.last_mut().expect("bits follow a unit's start");
                for index in (0..put.bits).rev() {
                    if unit.bits % 8 == 0 {
                        unit.payload.push(0);
                    }
                    let bit = ((put.value >> index) & 1) as u8;
                    *unit.payload.last_mut().unwrap() |= bit << (7 - unit.bits % 8);
                    unit.bits += 1;
                }
            }
        }
    }
    units
}

/// The stop bit and alignment after a slice's data, given the bits written
/// since the unit's header.
pub fn slice_trailing(bits_in_unit: usize) -> Put {
    let zeros = (8 - (bits_in_unit + 1) % 8) % 8;
    Put::new(1 << zeros, zeros as u8 + 1)
}

/// The stream's codec string for a decoder: `avc1.PPCCLL`.
pub fn codec_string(stream: &Stream) -> String {
    format!(
        "avc1.{:02x}{:02x}{:02x}",
        stream.profile_idc(),
        stream.constraint_flags(),
        stream.level_idc()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::{BitSink, BitSource};

    fn stream(width: u32, height: u32) -> Stream {
        Stream {
            width,
            height,
            fps: 30,
            cabac: false,
            chroma_qp_offset: 4,
            deblocking: true,
        }
    }

    /// The unit's payload as bytes, and its header byte.
    fn written(ops: &[HeaderOp]) -> (u8, Vec<u8>) {
        let mut sink = BitSink::new();
        let mut header = 0;
        for op in ops {
            match op {
                HeaderOp::Start(byte) => header = *byte,
                HeaderOp::Bits(put) => sink.put(*put),
            }
        }
        sink.align();
        (header, sink.into_bytes())
    }

    #[test]
    fn levels_follow_the_size_and_the_rate() {
        assert_eq!(stream(320, 240).level_idc(), 30);
        assert_eq!(stream(1280, 720).level_idc(), 31);
        assert_eq!(stream(1366, 768).level_idc(), 32);
        assert_eq!(stream(1920, 1080).level_idc(), 40);
        let fast = Stream {
            fps: 60,
            ..stream(1920, 1080)
        };
        assert_eq!(fast.level_idc(), 42);
        assert_eq!(codec_string(&stream(1920, 1080)), "avc1.42c028");
    }

    #[test]
    fn the_sps_reads_back_as_it_was_meant() {
        for (width, height) in [(320, 240), (1280, 720), (1366, 768), (1920, 1080)] {
            let stream = stream(width, height);
            let ops = sps(&stream);
            let (header, bytes) = written(&ops);
            assert_eq!(header, 0x67);
            let mut source = BitSource::new(&bytes);
            assert_eq!(source.bits(8), Some(66));
            assert_eq!(source.bits(8), Some(0xc0));
            assert_eq!(source.bits(8), Some(stream.level_idc() as u32));
            assert_eq!(source.ue(), Some(0));
            assert_eq!(source.ue(), Some(4));
            assert_eq!(source.ue(), Some(2));
            assert_eq!(source.ue(), Some(1));
            assert_eq!(source.bit(), Some(0));
            let width_mbs = source.ue().unwrap() + 1;
            let height_mbs = source.ue().unwrap() + 1;
            assert_eq!(source.bits(2), Some(3));
            let (mut right, mut bottom) = (0, 0);
            if source.bit() == Some(1) {
                assert_eq!(source.ue(), Some(0));
                right = source.ue().unwrap();
                assert_eq!(source.ue(), Some(0));
                bottom = source.ue().unwrap();
            }
            assert_eq!(width_mbs * 16 - right * 2, width);
            assert_eq!(height_mbs * 16 - bottom * 2, height);
            // The VUI: no flag but the bitstream restriction.
            assert_eq!(source.bits(10), Some(0b10_0000_0001));
            assert_eq!(source.bit(), Some(1));
            assert_eq!(source.ue(), Some(0));
            assert_eq!(source.ue(), Some(0));
            assert_eq!(source.ue(), Some(16));
            assert_eq!(source.ue(), Some(16));
            assert_eq!(source.ue(), Some(0));
            assert_eq!(source.ue(), Some(1));
            // The stop bit, then zeros to the end.
            assert_eq!(source.bit(), Some(1));
            assert_eq!(source.position().div_ceil(8), bytes.len());
            while let Some(bit) = source.bit() {
                assert_eq!(bit, 0);
            }
        }
    }

    #[test]
    fn the_pps_reads_back_as_it_was_meant() {
        let (header, bytes) = written(&pps(&stream(1920, 1080)));
        assert_eq!(header, 0x68);
        let mut source = BitSource::new(&bytes);
        assert_eq!(source.ue(), Some(0));
        assert_eq!(source.ue(), Some(0));
        assert_eq!(source.bits(2), Some(0));
        assert_eq!(source.ue(), Some(0));
        assert_eq!(source.ue(), Some(0));
        assert_eq!(source.ue(), Some(0));
        assert_eq!(source.bits(3), Some(0));
        assert_eq!(source.se(), Some(0));
        assert_eq!(source.se(), Some(0));
        assert_eq!(source.se(), Some(4));
        assert_eq!(source.bits(3), Some(0b100));
        assert_eq!(source.bit(), Some(1));
    }

    #[test]
    fn slice_headers_differ_as_key_and_p_frames_do() {
        let stream = stream(1280, 720);
        let key = Slice {
            key: true,
            frame_num: 0,
            idr_pic_id: 1,
            qp: 30,
            cabac_init_idc: 0,
        };
        let (header, bytes) = written(&slice_header(&stream, &key));
        assert_eq!(header, 0x65);
        let mut source = BitSource::new(&bytes);
        assert_eq!(source.ue(), Some(0));
        assert_eq!(source.ue(), Some(7));
        assert_eq!(source.ue(), Some(0));
        assert_eq!(source.bits(8), Some(0));
        assert_eq!(source.ue(), Some(1));
        assert_eq!(source.bits(2), Some(0));
        assert_eq!(source.se(), Some(4));
        assert_eq!(source.ue(), Some(0));
        assert_eq!(source.se(), Some(0));
        assert_eq!(source.se(), Some(0));

        let later = Slice {
            key: false,
            frame_num: 259,
            idr_pic_id: 1,
            qp: 22,
            cabac_init_idc: 0,
        };
        let (header, bytes) = written(&slice_header(&stream, &later));
        assert_eq!(header, 0x41);
        let mut source = BitSource::new(&bytes);
        assert_eq!(source.ue(), Some(0));
        assert_eq!(source.ue(), Some(5));
        assert_eq!(source.ue(), Some(0));
        assert_eq!(source.bits(8), Some(3));
        assert_eq!(source.bits(3), Some(0));
        assert_eq!(source.se(), Some(-4));
        assert_eq!(source.ue(), Some(0));
    }

    #[test]
    fn a_cabac_stream_says_so_and_names_the_start_table() {
        let stream = Stream {
            cabac: true,
            ..stream(1280, 720)
        };
        assert_eq!(codec_string(&stream), "avc1.4d401f");
        let slice = Slice {
            key: false,
            frame_num: 1,
            idr_pic_id: 0,
            qp: 26,
            cabac_init_idc: 1,
        };
        let (_, bytes) = written(&slice_header(&stream, &slice));
        let mut source = BitSource::new(&bytes);
        assert_eq!(source.ue(), Some(0));
        assert_eq!(source.ue(), Some(5));
        assert_eq!(source.ue(), Some(0));
        assert_eq!(source.bits(8), Some(1));
        assert_eq!(source.bits(3), Some(0));
        assert_eq!(source.ue(), Some(1));
        assert_eq!(source.se(), Some(0));
    }

    #[test]
    fn units_are_the_same_bits_in_pieces_of_sixteen() {
        let stream = Stream {
            width: 1366,
            height: 768,
            fps: 30,
            cabac: false,
            chroma_qp_offset: 0,
            deblocking: true,
        };
        let slice = Slice {
            key: true,
            frame_num: 0,
            idr_pic_id: 1,
            qp: 30,
            cabac_init_idc: 0,
        };
        let mut ops = sps(&stream);
        ops.extend(pps(&stream));
        ops.extend(slice_header(&stream, &slice));
        let units = units(&ops);
        assert_eq!(
            units.iter().map(|unit| unit.header).collect::<Vec<_>>(),
            [0x67, 0x68, 0x65]
        );
        // The parameter sets end on a byte; the slice header does not have to.
        assert_eq!(units[0].bits % 8, 0);
        assert_eq!(units[1].bits % 8, 0);
        assert!(
            units
                .iter()
                .all(|unit| unit.payload.len() == unit.bits.div_ceil(8))
        );
        // Written again from the units, the stream is the same.
        let again: Vec<HeaderOp> = units.iter().flat_map(RawUnit::ops).collect();
        assert_eq!(bytes(&again), bytes(&ops));
        assert!(again.iter().all(|op| match op {
            HeaderOp::Bits(put) => put.bits <= 16,
            HeaderOp::Start(_) => true,
        }));
    }

    #[test]
    fn parameter_sets_are_whole_bytes_behind_start_codes() {
        let stream = stream(1920, 1080);
        let mut ops = sps(&stream);
        let sps_bytes = bytes(&ops);
        assert_eq!(sps_bytes[..5], [0, 0, 0, 1, 0x67]);
        assert_eq!(sps_bytes[5..8], [66, 0xc0, 40]);
        ops.extend(pps(&stream));
        let both = bytes(&ops);
        assert_eq!(both[..sps_bytes.len()], sps_bytes[..]);
        assert_eq!(
            both[sps_bytes.len()..sps_bytes.len() + 5],
            [0, 0, 0, 1, 0x68]
        );
        // Nothing in a payload looks like a start code.
        assert_eq!(
            both.windows(3)
                .filter(|window| *window == [0, 0, 1])
                .count(),
            2
        );
    }

    #[test]
    fn trailing_bits_end_on_a_byte() {
        for bits in 0..64 {
            let put = slice_trailing(bits);
            assert_eq!((bits + put.bits as usize) % 8, 0);
            assert_eq!(put.value, 1 << (put.bits - 1));
        }
    }
}
