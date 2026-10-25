//! A frame that says "nothing changed": a P slice whose macroblocks are all
//! skipped, made in software. A stream gets one in the place of a frame its
//! rate has no room for; a decoder shows the picture before it.

use crate::{
    bits::{BitSink, Put},
    headers::{HeaderOp, Slice, Stream, slice_header},
};

/// A whole frame that says "nothing changed": a P slice whose macroblocks
/// are all skipped, made here and not by the engine. A stream gets one in
/// the place of a frame its rate has no room for. CAVLC only.
pub fn unchanged_frame(stream: &Stream, frame_num: u32) -> Vec<u8> {
    assert!(!stream.cabac, "a skipped frame is written for CAVLC only");
    let slice = Slice {
        key: false,
        frame_num,
        idr_pic_id: 0,
        // Nothing is quantised: the header's QP is the PPS's.
        qp: crate::headers::INITIAL_QP,
        cabac_init_idc: 0,
    };
    let mut sink = BitSink::new();
    for op in slice_header(stream, &slice) {
        match op {
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
    // `mb_skip_run`: every macroblock of the picture. Then the stop bit,
    // and zeros to the byte's end.
    sink.put(Put::ue(stream.macroblocks()));
    sink.put(Put::new(1, 1));
    sink.align();
    sink.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::{BitSource, without_prevention};

    #[test]
    fn an_unchanged_frame_is_a_p_slice_of_skipped_macroblocks() {
        let stream = Stream {
            width: 1920,
            height: 1080,
            fps: 30,
            cabac: false,
            chroma_qp_offset: 0,
            deblocking: true,
        };
        let frame = unchanged_frame(&stream, 7);
        assert_eq!(frame[..5], [0, 0, 0, 1, 0x41]);
        assert!(frame.len() < 16);
        let payload = without_prevention(&frame[5..]);
        let mut source = BitSource::new(&payload);
        assert_eq!(source.ue(), Some(0)); // first_mb_in_slice
        assert_eq!(source.ue(), Some(5)); // P, as every slice of the picture
        assert_eq!(source.ue(), Some(0)); // pic_parameter_set_id
        assert_eq!(source.bits(8), Some(7)); // frame_num
        assert_eq!(source.bits(1), Some(0)); // num_ref_idx_active_override_flag
        assert_eq!(source.bits(1), Some(0)); // ref_pic_list_modification_flag_l0
        assert_eq!(source.bits(1), Some(0)); // adaptive_ref_pic_marking_mode_flag
        assert_eq!(source.se(), Some(0)); // slice_qp_delta
        assert_eq!(source.ue(), Some(0)); // disable_deblocking_filter_idc
        assert_eq!(source.se(), Some(0)); // slice_alpha_c0_offset_div2
        assert_eq!(source.se(), Some(0)); // slice_beta_offset_div2
        // Every macroblock of 120 by 68 skipped, and the unit ends.
        assert_eq!(source.ue(), Some(8160));
        assert_eq!(source.bits(1), Some(1));
    }
}
