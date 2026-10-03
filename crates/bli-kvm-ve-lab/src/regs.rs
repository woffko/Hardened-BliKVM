//! The engine's registers as the open sources describe them, and the plan of
//! an encode as a list of register operations. Hypotheses stand for the two
//! open maps and for what the vendor's library was seen to write for the
//! generation before this engine's (`docs/design/h616-ve-h264-encoder.md`);
//! each of their differences is a field, so that a lab run can change one at
//! a time.

use crate::{
    bits::Put,
    headers::{HeaderOp, Slice, Stream},
};

pub const MODE: u32 = 0x000;
pub const RESET: u32 = 0x004;
pub const VERSION: u32 = 0x0f0;

pub const MODE_DECODER_NONE: u32 = 7;
pub const MODE_DECODER_FIELD: u32 = 0xf;
pub const MODE_INPUT_STAGE: u32 = 1 << 6;
pub const MODE_ENCODER: u32 = 1 << 7;
/// The memory interface bits both sources set for other engines.
pub const MODE_MEMORY: u32 = 0x0013_0000;
/// The memory interface field and the reconstruction write mode.
pub const MODE_MEMORY_FIELD: u32 = 0x0013_0000;
/// What the vendor's keeper sets on the H6: the field at 2.
pub const MODE_MEMORY_H6: u32 = 2 << 16;
pub const RESET_ENCODER: u32 = 1 << 24;
pub const RESET_IDLE: u32 = (1 << 8) | (1 << 9);

pub const ISP_SIZE: u32 = 0xa00;
pub const ISP_STRIDE: u32 = 0xa04;
pub const ISP_CTRL: u32 = 0xa08;
pub const ISP_STATUS: u32 = 0xa10;
/// The input stride in macroblocks a second time (the vendor's library).
pub const ISP_STRIDE_MBS: u32 = 0xa14;
pub const ISP_SCALER_SIZE: u32 = 0xa2c;
pub const ISP_INPUT_LUMA: u32 = 0xa78;
pub const ISP_INPUT_CHROMA: u32 = 0xa7c;
/// Where a second chroma plane would be.
pub const ISP_INPUT_CHROMA2: u32 = 0xa80;

pub const AVC_PARA0: u32 = 0xb04;
pub const AVC_PARA1: u32 = 0xb08;
pub const AVC_PARA2: u32 = 0xb0c;
pub const AVC_ME: u32 = 0xb10;
pub const AVC_INT_EN: u32 = 0xb14;
pub const AVC_TRIGGER: u32 = 0xb18;
pub const AVC_STATUS: u32 = 0xb1c;
pub const AVC_PUT_DATA: u32 = 0xb20;
pub const AVC_OVERTIME_MB: u32 = 0xb24;
pub const AVC_INTRA_REFRESH: u32 = 0xb28;
pub const AVC_RC_INIT: u32 = 0xb2c;
pub const AVC_RC_MAD_TH0: u32 = 0xb30;
pub const AVC_RC_MAD_SUM: u32 = 0xb40;
pub const AVC_DYNAMIC_ME0: u32 = 0xb48;
pub const AVC_DYNAMIC_ME1: u32 = 0xb4c;
pub const AVC_MAD: u32 = 0xb50;
pub const AVC_RESIDUAL_BITS: u32 = 0xb54;
pub const AVC_HEADER_BITS: u32 = 0xb58;
pub const AVC_ME_INFO: u32 = 0xb5c;
pub const AVC_MV_BUFFER: u32 = 0xb60;
pub const AVC_STREAM_START: u32 = 0xb80;
pub const AVC_STREAM_END: u32 = 0xb84;
pub const AVC_STREAM_OFFSET: u32 = 0xb88;
pub const AVC_STREAM_MAX: u32 = 0xb8c;
pub const AVC_STREAM_LENGTH: u32 = 0xb90;
pub const AVC_REF_LUMA: u32 = 0xba0;
pub const AVC_REF_CHROMA: u32 = 0xba4;
pub const AVC_REC_LUMA: u32 = 0xbb0;
pub const AVC_REC_CHROMA: u32 = 0xbb4;
pub const AVC_SUBPIXEL_LAST: u32 = 0xbb8;
pub const AVC_SUBPIXEL_NEW: u32 = 0xbbc;
pub const AVC_MB_INFO: u32 = 0xbc0;
pub const AVC_DEBLOCK: u32 = 0xbc4;

pub const PARA0_P_SLICE: u32 = 1 << 4;
pub const PARA0_CABAC: u32 = 1 << 8;
/// Set by the vendor's library in every P frame of the newer generation;
/// what it means is not known (the V3s map names bits 11:10 "fix mode num"
/// and says no more).
pub const PARA0_NEWER_P: u32 = 1 << 10;
pub const PARA0_NO_DEBLOCKING: u32 = 1 << 12;
pub const PARA0_NO_PREVENTION: u32 = 1 << 31;
pub const PARA1_DYNAMIC_ME: u32 = 1 << 8;
/// Set by the vendor's library in every frame of the newer generation; what
/// it means is not known.
pub const PARA1_NEWER: u32 = 1 << 27;
/// What the vendor's library writes into the motion estimation register
/// of the newer generation: no write-back of motion vectors (bit 16) and
/// bit 12.
pub const ME_NEWER: u32 = 0x0001_1000;
/// "QSMTSS refine off" in the open map. With it this engine finds a
/// picture that moves down by more than 12 pixels a frame (V3.4's
/// addendum in the design note); the vendor's library leaves it clear.
pub const ME_REFINE_OFF: u32 = 1 << 4;
pub const TRIGGER_PUT_BITS: u32 = 1;
/// Fills the stream up to a whole byte; the vendor's library ends headers
/// that stand alone with it.
pub const TRIGGER_ALIGN: u32 = 2;
pub const TRIGGER_START: u32 = 8;
pub const STATUS_FINISH: u32 = 1 << 0;
pub const STATUS_STALL: u32 = 1 << 1;
pub const STATUS_ENDED: u32 = 0xf;
pub const STATUS_PUT_READY: u32 = 1 << 9;

/// The names of the registers a plan touches, for receipts and the log.
pub fn name(reg: u32) -> &'static str {
    match reg {
        MODE => "mode",
        RESET => "reset",
        VERSION => "version",
        ISP_SIZE => "isp.size",
        ISP_STRIDE => "isp.stride",
        ISP_CTRL => "isp.ctrl",
        ISP_STATUS => "isp.status",
        ISP_STRIDE_MBS => "isp.stride_mbs",
        ISP_SCALER_SIZE => "isp.scaler_size",
        ISP_INPUT_LUMA => "isp.input_luma",
        ISP_INPUT_CHROMA => "isp.input_chroma",
        ISP_INPUT_CHROMA2 => "isp.input_chroma2",
        AVC_PARA0 => "avc.para0",
        AVC_PARA1 => "avc.para1",
        AVC_PARA2 => "avc.para2",
        AVC_ME => "avc.me",
        AVC_INT_EN => "avc.int_en",
        AVC_TRIGGER => "avc.trigger",
        AVC_STATUS => "avc.status",
        AVC_PUT_DATA => "avc.put_data",
        AVC_OVERTIME_MB => "avc.overtime_mb",
        AVC_INTRA_REFRESH => "avc.intra_refresh",
        AVC_RC_INIT => "avc.rc_init",
        0xb30 => "avc.rc_mad_th0",
        0xb34 => "avc.rc_mad_th1",
        0xb38 => "avc.rc_mad_th2",
        0xb3c => "avc.rc_mad_th3",
        AVC_RC_MAD_SUM => "avc.rc_mad_sum",
        0xb44 => "avc.temporal_filter",
        AVC_DYNAMIC_ME0 => "avc.dynamic_me0",
        AVC_DYNAMIC_ME1 => "avc.dynamic_me1",
        AVC_MAD => "avc.mad",
        AVC_RESIDUAL_BITS => "avc.residual_bits",
        AVC_HEADER_BITS => "avc.header_bits",
        AVC_ME_INFO => "avc.me_info",
        AVC_MV_BUFFER => "avc.mv_buffer",
        AVC_STREAM_START => "avc.stream_start",
        AVC_STREAM_END => "avc.stream_end",
        AVC_STREAM_OFFSET => "avc.stream_offset",
        AVC_STREAM_MAX => "avc.stream_max",
        AVC_STREAM_LENGTH => "avc.stream_length",
        AVC_REF_LUMA => "avc.ref_luma",
        AVC_REF_CHROMA => "avc.ref_chroma",
        0xba8 => "avc.ref1_luma",
        0xbac => "avc.ref1_chroma",
        AVC_REC_LUMA => "avc.rec_luma",
        AVC_REC_CHROMA => "avc.rec_chroma",
        AVC_SUBPIXEL_LAST => "avc.subpixel_last",
        AVC_SUBPIXEL_NEW => "avc.subpixel_new",
        AVC_MB_INFO => "avc.mb_info",
        AVC_DEBLOCK => "avc.deblock",
        0xbc8 => "avc.temporal_filter_counts",
        _ => "",
    }
}

/// The registers of the blocks the open maps name: what the mask probe may
/// write, never a trigger among them.
pub const MASK_PROBE: [u32; 34] = [
    ISP_SIZE,
    ISP_STRIDE,
    ISP_CTRL,
    ISP_SCALER_SIZE,
    ISP_INPUT_LUMA,
    ISP_INPUT_CHROMA,
    AVC_PARA0,
    AVC_PARA1,
    AVC_PARA2,
    AVC_ME,
    AVC_OVERTIME_MB,
    AVC_INTRA_REFRESH,
    AVC_RC_INIT,
    0xb30,
    0xb34,
    0xb38,
    0xb3c,
    0xb44,
    AVC_DYNAMIC_ME0,
    AVC_DYNAMIC_ME1,
    AVC_MV_BUFFER,
    AVC_STREAM_START,
    AVC_STREAM_END,
    AVC_STREAM_OFFSET,
    AVC_STREAM_MAX,
    AVC_REF_LUMA,
    AVC_REF_CHROMA,
    AVC_REC_LUMA,
    AVC_REC_CHROMA,
    AVC_SUBPIXEL_LAST,
    AVC_SUBPIXEL_NEW,
    AVC_MB_INFO,
    AVC_DEBLOCK,
    0xbc8,
];

/// One step of a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Write {
        reg: u32,
        value: u32,
    },
    /// Reads the register, clears `clear`, sets `set`, writes it back.
    Update {
        reg: u32,
        clear: u32,
        set: u32,
    },
    /// Waits until every bit of `mask` reads as set.
    Until {
        reg: u32,
        mask: u32,
    },
    /// Waits for the encode to end.
    Wait,
}

pub use bli_kvm_ve_encoder::rate::Search;

/// What a search makes of the motion estimation register.
pub fn search_adjust(search: Search) -> Adjust {
    Adjust {
        reg: AVC_ME,
        clear: ME_REFINE_OFF,
        set: match search {
            Search::Exact => 0,
            Search::Wide => ME_REFINE_OFF,
        },
    }
}

/// A change of fields in what every frame's plan writes into one register:
/// the lab's way to try what a field of the open map does to this engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Adjust {
    pub reg: u32,
    pub clear: u32,
    pub set: u32,
}

/// The bits of `reg` a lab run may change: fields the open map names that
/// say how the engine searches and decides. None says where or whether it
/// writes to memory (the write-back of motion vectors, the region of
/// interest and the two "to memory" switches of `0xb10` stay as the plan
/// has them), and none is without a name.
pub fn adjustable(reg: u32) -> u32 {
    match reg {
        // Mode optimisation 31:28, motion vector clipping 26:24, dynamic
        // motion estimation bit 8.
        AVC_PARA1 => 0xf700_0100,
        // Bits 0 to 9 and 11 to 15, and the quarter-pixel switches 21:20.
        AVC_ME => 0x0030_fbff,
        // The four thresholds.
        AVC_DYNAMIC_ME0 | AVC_DYNAMIC_ME1 => 0x03ff_03ff,
        _ => 0,
    }
}

impl Adjust {
    /// None when the change reaches a bit that is not `adjustable`.
    pub fn new(reg: u32, clear: u32, set: u32) -> Option<Self> {
        ((clear | set) & !adjustable(reg) == 0).then_some(Self { reg, clear, set })
    }

    fn apply(&self, value: u32) -> u32 {
        (value & !self.clear) | self.set
    }
}

/// `ops` with every value written to an adjusted register changed.
pub fn adjusted(mut ops: Vec<Op>, adjusts: &[Adjust]) -> Vec<Op> {
    for op in &mut ops {
        for adjust in adjusts {
            match op {
                Op::Write { reg, value } if *reg == adjust.reg => *value = adjust.apply(*value),
                Op::Update { reg, clear, set } if *reg == adjust.reg => {
                    *clear |= adjust.clear;
                    *set = adjust.apply(*set);
                }
                _ => {}
            }
        }
    }
    ops
}

/// How the encoder is selected in the mode register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Select {
    /// Engine 11 in the low four bits (the older map).
    Engine11,
    /// Bits 7 and 6 with no decoder (the newer map and the vendor kernel).
    EncoderBits,
}

/// What `0xb08` holds besides the QP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Para1 {
    /// The QP twice, at 5:0 and 13:8.
    TwoQps,
    /// The QP at 5:0 and the input stride divided by 48 at 13:10.
    StrideDiv48,
    /// The same two fields, dynamic motion estimation on, bit 27 set, and
    /// no chroma QP offset: what the vendor's library writes for the H5 and
    /// the H6.
    Newer,
}

/// One reading of the engine: every difference between the open maps is a
/// field here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hypothesis {
    pub name: &'static str,
    pub select: Select,
    /// Pulse the encoder's reset bit when selecting it.
    pub reset_encoder: bool,
    /// Wait for the bit writer before every put.
    pub wait_put_ready: bool,
    /// Wait for the idle bits after the headers.
    pub wait_idle_after_headers: bool,
    /// Give the input stage's scaler the picture's size.
    pub scaler_size: bool,
    pub para1: Para1,
    /// The value of the motion estimation register.
    pub me: u32,
    /// Give `0xb60` a buffer; otherwise write 0 there.
    pub extra_buffer: bool,
    /// The interrupt enables.
    pub interrupts: u32,
    /// Write zero to the statistics, rate control and optional registers
    /// before every frame.
    pub clear_first: bool,
    /// Give a key frame its own reconstruction as its reference.
    pub key_references_itself: bool,
    /// What is subtracted from a buffer's bus address before the engine is
    /// given it: nothing, or the address at which the memory begins, as the
    /// older generations count.
    pub address_base: u32,
    /// The encoder block's address registers hold the address shifted right
    /// by this many bits: 0, or 8 for units of 256 bytes. The input stage's
    /// registers hold the address as it is in every generation.
    pub block_shift: u32,
    /// The input stage is told the picture's size in units of 8 pixels
    /// instead of macroblocks, its stride a second time, and where a second
    /// chroma plane would be.
    pub input_in_eighths: bool,
    /// The order of a frame's writes is the vendor library's: input stage,
    /// first parameters, stream, buffers, headers, last parameters, start.
    /// Otherwise Bootlin's: stream, headers, input stage, buffers and
    /// parameters, start.
    pub input_first: bool,
    /// The engine is reset and the encoder selected again before every
    /// frame, as the vendor's library asks of its keeper.
    pub reset_each_frame: bool,
    /// Headers that stand alone end with the align trigger.
    pub align_after_headers: bool,
    /// What the select writes into the mode register's memory interface
    /// field; none leaves what the reset left.
    pub memory_mode: Option<u32>,
}

/// Where the board's memory begins.
pub const MEMORY_BASE: u32 = 0x4000_0000;

impl Hypothesis {
    /// The A10 to H3 generations, after jemk's `h264enc`.
    pub const OLDER: Hypothesis = Hypothesis {
        name: "older",
        select: Select::Engine11,
        reset_encoder: false,
        wait_put_ready: false,
        wait_idle_after_headers: false,
        scaler_size: false,
        para1: Para1::TwoQps,
        me: 0x0000_0104,
        extra_buffer: true,
        interrupts: 0xf,
        clear_first: false,
        key_references_itself: false,
        address_base: 0,
        block_shift: 0,
        input_in_eighths: false,
        input_first: false,
        reset_each_frame: false,
        align_after_headers: false,
        memory_mode: None,
    };

    /// The V3, V3s and S3, after Bootlin's encoder.
    pub const V3S: Hypothesis = Hypothesis {
        name: "v3s",
        select: Select::EncoderBits,
        reset_encoder: true,
        wait_put_ready: true,
        wait_idle_after_headers: true,
        scaler_size: true,
        para1: Para1::StrideDiv48,
        me: 0x0001_0004,
        extra_buffer: false,
        interrupts: 0x3,
        clear_first: true,
        key_references_itself: true,
        address_base: 0,
        block_shift: 0,
        input_in_eighths: false,
        input_first: false,
        reset_each_frame: false,
        align_after_headers: false,
        memory_mode: None,
    };

    /// The newer map with addresses counted from the memory's beginning.
    pub const V3S_RELATIVE: Hypothesis = Hypothesis {
        name: "v3s-relative",
        address_base: MEMORY_BASE,
        ..Self::V3S
    };

    /// The H5 and the H6, after what the vendor's encoder library wrote for
    /// them behind the register trace (V2R). The H616's encoder is one
    /// revision on from the H6's.
    pub const H6: Hypothesis = Hypothesis {
        name: "h6",
        select: Select::EncoderBits,
        reset_encoder: true,
        wait_put_ready: true,
        wait_idle_after_headers: false,
        scaler_size: true,
        para1: Para1::Newer,
        me: ME_NEWER,
        extra_buffer: false,
        interrupts: 0xf,
        clear_first: true,
        key_references_itself: true,
        address_base: 0,
        block_shift: 8,
        input_in_eighths: true,
        input_first: true,
        reset_each_frame: true,
        align_after_headers: true,
        memory_mode: Some(MODE_MEMORY_H6),
    };

    /// The same with the memory interface field as the reset leaves it.
    pub const H6_PLAIN_MODE: Hypothesis = Hypothesis {
        name: "h6-plain-mode",
        memory_mode: None,
        ..Self::H6
    };

    /// The same with the field as `cedrus` sets it for this board's
    /// decoders.
    pub const H6_DECODER_MODE: Hypothesis = Hypothesis {
        name: "h6-decoder-mode",
        memory_mode: Some(MODE_MEMORY),
        ..Self::H6
    };

    /// This board's engine as the plan drives it: the H6's form with one
    /// bit more in the motion estimation register.
    pub const H616: Hypothesis = Hypothesis {
        name: "h616",
        me: ME_NEWER | ME_REFINE_OFF,
        ..Self::H6
    };

    pub fn named(name: &str) -> Option<Hypothesis> {
        [
            Self::OLDER,
            Self::V3S,
            Self::V3S_RELATIVE,
            Self::H6,
            Self::H6_PLAIN_MODE,
            Self::H6_DECODER_MODE,
            Self::H616,
        ]
        .into_iter()
        .find(|hypothesis| hypothesis.name == name)
    }

    /// An address as an address register of the encoder block holds it.
    pub fn block(&self, address: u32) -> u32 {
        address >> self.block_shift
    }

    /// `chroma_qp_index_offset` of a stream this engine encodes: where the
    /// parameters have no field for it, the vendor's library says none.
    pub fn chroma_qp_offset(&self) -> i32 {
        if self.para1 == Para1::Newer { 0 } else { 4 }
    }
}

/// The buffers of one frame that will be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reconstruction {
    pub luma: u32,
    pub chroma: u32,
    pub subpixel: u32,
}

/// Bus addresses of everything the engine reads or writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buffers {
    pub input_luma: u32,
    pub input_chroma: u32,
    pub stream: u32,
    pub stream_bytes: u32,
    pub reconstruction: [Reconstruction; 2],
    pub macroblock_info: u32,
    pub extra: u32,
}

/// The bytes the buffers of a stream need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sizes {
    pub input: usize,
    pub reconstruction_luma: usize,
    pub reconstruction_chroma: usize,
    pub subpixel: usize,
    pub macroblock_info: usize,
    pub extra: usize,
}

fn align(value: usize, to: usize) -> usize {
    value.div_ceil(to) * to
}

impl Sizes {
    /// As jemk's encoder allocates.
    pub fn older(stream: &Stream) -> Self {
        let (columns, rows) = (stream.width_mbs() as usize, stream.height_mbs() as usize);
        let luma = align(columns * 16, 32) * align(rows * 16, 32);
        Self {
            input: columns * 16 * rows * 16 * 3 / 2,
            reconstruction_luma: luma,
            reconstruction_chroma: align(columns * 16, 32) * align(rows * 8, 32),
            subpixel: luma / 4,
            macroblock_info: columns * 32,
            extra: align(columns, 4) * rows * 8,
        }
    }

    /// As Bootlin's encoder allocates.
    pub fn v3s(stream: &Stream) -> Self {
        let (columns, rows) = (stream.width_mbs() as usize, stream.height_mbs() as usize);
        let subpixel_width = ((columns + 47) * 2 / 3) / 32 * 32 + align(columns, 32) * 2;
        let subpixel_height = (rows * 16 + 72) / 8;
        Self {
            input: columns * 16 * rows * 16 * 3 / 2,
            reconstruction_luma: align(columns, 2) * 16 * align(rows + 1, 4) * 16,
            reconstruction_chroma: align(columns, 2) * 16 * align(rows.div_ceil(2), 4) * 16,
            subpixel: subpixel_width * subpixel_height,
            macroblock_info: columns.div_ceil(32) * 4096,
            extra: 0,
        }
    }

    /// What a driver would allocate (`kernel/hardened-blikvm-ve`): the
    /// V3s map's sizes for the picture itself, each buffer a whole number
    /// of pages and one page more. The lab uses them only after it has
    /// seen, in its own larger buffers, that the engine fills no more.
    pub fn tight(stream: &Stream) -> Self {
        let v3s = Self::v3s(stream);
        let page = |bytes: usize| align(bytes, 4096) + 4096;
        Self {
            input: v3s.input,
            reconstruction_luma: v3s.reconstruction_luma,
            reconstruction_chroma: page(v3s.reconstruction_luma + v3s.reconstruction_chroma)
                - v3s.reconstruction_luma,
            subpixel: page(v3s.subpixel),
            macroblock_info: v3s.macroblock_info + 4096,
            extra: 0,
        }
    }

    /// What the lab allocates: the larger of both for every buffer.
    pub fn lab(stream: &Stream) -> Self {
        let (older, v3s) = (Self::older(stream), Self::v3s(stream));
        Self {
            input: older.input.max(v3s.input),
            reconstruction_luma: older.reconstruction_luma.max(v3s.reconstruction_luma),
            reconstruction_chroma: older.reconstruction_chroma.max(v3s.reconstruction_chroma),
            subpixel: older.subpixel.max(v3s.subpixel),
            macroblock_info: older.macroblock_info.max(v3s.macroblock_info),
            extra: older.extra.max(v3s.extra),
        }
    }
}

/// Selecting the encoder, once per stream.
pub fn select(hypothesis: &Hypothesis) -> Vec<Op> {
    match hypothesis.select {
        Select::Engine11 => vec![Op::Write {
            reg: MODE,
            value: MODE_MEMORY | 0xb,
        }],
        Select::EncoderBits => {
            let mut ops = vec![Op::Update {
                reg: MODE,
                clear: MODE_ENCODER | MODE_INPUT_STAGE | MODE_DECODER_FIELD,
                set: MODE_DECODER_NONE,
            }];
            if hypothesis.reset_encoder {
                ops.push(Op::Update {
                    reg: RESET,
                    clear: 0,
                    set: RESET_ENCODER,
                });
                ops.push(Op::Update {
                    reg: RESET,
                    clear: RESET_ENCODER,
                    set: 0,
                });
            }
            ops.push(match hypothesis.memory_mode {
                None => Op::Update {
                    reg: MODE,
                    clear: 0,
                    set: MODE_ENCODER | MODE_INPUT_STAGE | MODE_DECODER_NONE,
                },
                Some(memory) => Op::Update {
                    reg: MODE,
                    clear: MODE_MEMORY_FIELD,
                    set: memory | MODE_ENCODER | MODE_INPUT_STAGE | MODE_DECODER_NONE,
                },
            });
            ops
        }
    }
}

/// Giving the engine back: no encoder, no decoder.
pub fn deselect(hypothesis: &Hypothesis) -> Vec<Op> {
    match hypothesis.select {
        Select::Engine11 => vec![Op::Write {
            reg: MODE,
            value: MODE_MEMORY | MODE_DECODER_NONE,
        }],
        Select::EncoderBits => vec![Op::Update {
            reg: MODE,
            clear: MODE_ENCODER | MODE_INPUT_STAGE | MODE_DECODER_FIELD,
            set: MODE_DECODER_NONE,
        }],
    }
}

fn put(hypothesis: &Hypothesis, put: Put, ops: &mut Vec<Op>) {
    for piece in put.pieces() {
        if hypothesis.wait_put_ready {
            ops.push(Op::Until {
                reg: AVC_STATUS,
                mask: STATUS_PUT_READY,
            });
        }
        ops.push(Op::Write {
            reg: AVC_PUT_DATA,
            value: piece.value,
        });
        ops.push(Op::Write {
            reg: AVC_TRIGGER,
            value: TRIGGER_PUT_BITS | (piece.bits as u32) << 8,
        });
    }
}

/// Headers through the engine's bit writer. A start code and the unit's
/// header byte go in with emulation prevention off, the rest with it on.
pub fn headers(hypothesis: &Hypothesis, header_ops: &[HeaderOp]) -> Vec<Op> {
    let mut ops = Vec::new();
    for op in header_ops {
        match *op {
            HeaderOp::Start(byte) => {
                ops.push(Op::Update {
                    reg: AVC_PARA0,
                    clear: 0,
                    set: PARA0_NO_PREVENTION,
                });
                put(hypothesis, Put::new(0, 16), &mut ops);
                put(hypothesis, Put::new(1, 16), &mut ops);
                put(hypothesis, Put::new(byte as u32, 8), &mut ops);
                ops.push(Op::Update {
                    reg: AVC_PARA0,
                    clear: PARA0_NO_PREVENTION,
                    set: 0,
                });
            }
            HeaderOp::Bits(bits) => put(hypothesis, bits, &mut ops),
        }
    }
    if hypothesis.wait_idle_after_headers {
        ops.push(Op::Until {
            reg: RESET,
            mask: RESET_IDLE,
        });
    }
    ops
}

/// The four registers that say where the engine writes the stream.
fn stream_registers(hypothesis: &Hypothesis, buffers: &Buffers) -> [Op; 4] {
    let write = |reg, value| Op::Write { reg, value };
    [
        write(AVC_STREAM_OFFSET, 0),
        write(AVC_STREAM_START, hypothesis.block(buffers.stream)),
        write(
            AVC_STREAM_END,
            hypothesis.block(buffers.stream + buffers.stream_bytes - 1),
        ),
        write(AVC_STREAM_MAX, buffers.stream_bytes * 8),
    ]
}

/// Where the engine writes the stream, before the frame's headers.
pub fn stream_buffer(hypothesis: &Hypothesis, buffers: &Buffers) -> Vec<Op> {
    let mut ops = Vec::new();
    if hypothesis.clear_first {
        ops.push(Op::Write {
            reg: AVC_TRIGGER,
            value: 0,
        });
    }
    ops.extend(stream_registers(hypothesis, buffers));
    if hypothesis.clear_first && !hypothesis.input_first {
        for reg in [AVC_STREAM_LENGTH, AVC_HEADER_BITS, AVC_RESIDUAL_BITS] {
            ops.push(Op::Write { reg, value: 0 });
        }
    }
    ops
}

/// The end of headers that stand alone, where the engine is asked for it.
pub fn headers_end(hypothesis: &Hypothesis) -> Vec<Op> {
    if hypothesis.align_after_headers {
        vec![Op::Write {
            reg: AVC_TRIGGER,
            value: TRIGGER_ALIGN,
        }]
    } else {
        Vec::new()
    }
}

/// The picture to encode: its size, stride, format and place.
pub fn input(hypothesis: &Hypothesis, stream: &Stream, stride: u32, buffers: &Buffers) -> Vec<Op> {
    let write = |reg, value| Op::Write { reg, value };
    if hypothesis.input_in_eighths {
        // The visible size in units of 8 pixels, and the stride twice.
        let (width, height) = (stream.width.div_ceil(8), stream.height.div_ceil(8));
        return vec![
            write(ISP_SIZE, width << 16 | height),
            write(ISP_STRIDE, (stride / 16) << 16),
            write(ISP_STRIDE_MBS, stride / 16),
            write(ISP_SCALER_SIZE, height << 16 | width),
            write(ISP_CTRL, 0),
            write(ISP_INPUT_LUMA, buffers.input_luma),
            write(ISP_INPUT_CHROMA, buffers.input_chroma),
            write(
                ISP_INPUT_CHROMA2,
                buffers.input_chroma + stride * stream.height / 4,
            ),
        ];
    }
    let (columns, rows) = (stream.width_mbs(), stream.height_mbs());
    let mut ops = vec![write(ISP_SIZE, columns << 16 | rows)];
    if hypothesis.scaler_size {
        ops.push(write(ISP_SCALER_SIZE, rows << 16 | columns));
    }
    ops.push(write(ISP_STRIDE, (stride / 16) << 16));
    // NV12, no rotation, BT.601.
    ops.push(write(ISP_CTRL, 0));
    ops.push(write(ISP_INPUT_LUMA, buffers.input_luma));
    ops.push(write(ISP_INPUT_CHROMA, buffers.input_chroma));
    ops
}

/// The frame itself: its buffers, its parameters, the trigger, the wait.
/// `index` counts the frames of the stream; the two sets of reconstruction
/// buffers take turns.
pub fn encode(
    hypothesis: &Hypothesis,
    stream: &Stream,
    stride: u32,
    slice: &Slice,
    index: u32,
    buffers: &Buffers,
) -> Vec<Op> {
    let own = buffers.reconstruction[(index % 2) as usize];
    let last = buffers.reconstruction[((index + 1) % 2) as usize];
    let write = |reg, value| Op::Write { reg, value };
    let block = |reg, address| Op::Write {
        reg,
        value: hypothesis.block(address),
    };
    let mut ops = vec![
        block(AVC_MB_INFO, buffers.macroblock_info),
        if hypothesis.extra_buffer {
            block(AVC_MV_BUFFER, buffers.extra)
        } else {
            write(AVC_MV_BUFFER, 0)
        },
        block(AVC_REC_LUMA, own.luma),
        block(AVC_REC_CHROMA, own.chroma),
    ];
    if !slice.key {
        ops.push(block(AVC_REF_LUMA, last.luma));
        ops.push(block(AVC_REF_CHROMA, last.chroma));
    } else if hypothesis.key_references_itself {
        ops.push(block(AVC_REF_LUMA, own.luma));
        ops.push(block(AVC_REF_CHROMA, own.chroma));
    }
    ops.push(block(AVC_SUBPIXEL_NEW, own.subpixel));
    if !slice.key || hypothesis.key_references_itself {
        let subpixel = if slice.key {
            own.subpixel
        } else {
            last.subpixel
        };
        ops.push(block(AVC_SUBPIXEL_LAST, subpixel));
    }
    if hypothesis.clear_first {
        ops.push(write(AVC_DEBLOCK, 0));
        ops.push(write(AVC_INTRA_REFRESH, 0));
    }

    let mut para0 = 0;
    if !slice.key {
        para0 |= PARA0_P_SLICE;
    }
    if stream.cabac {
        para0 |= PARA0_CABAC;
    }
    if hypothesis.para1 == Para1::StrideDiv48 {
        if stream.cabac && !slice.key {
            para0 |= (slice.cabac_init_idc & 3) << 10;
        }
        if !stream.deblocking {
            para0 |= PARA0_NO_DEBLOCKING;
        }
    }
    ops.push(write(AVC_PARA0, para0));
    let qp = slice.qp as u32 & 0x3f;
    let chroma = (stream.chroma_qp_offset as u32 & 7) << 16;
    ops.push(write(
        AVC_PARA1,
        match hypothesis.para1 {
            Para1::TwoQps => chroma | qp << 8 | qp,
            Para1::StrideDiv48 => chroma | (stride / 16).div_ceil(48) << 10 | qp,
            Para1::Newer => newer_para1(stride, qp),
        },
    ));
    if hypothesis.clear_first {
        ops.push(write(AVC_PARA2, 0));
        ops.push(write(AVC_DYNAMIC_ME0, 0));
        ops.push(write(AVC_DYNAMIC_ME1, 0));
        for offset in 0..5 {
            ops.push(write(AVC_RC_INIT + 4 * offset, 0));
        }
    }
    ops.push(write(AVC_ME, hypothesis.me));
    if hypothesis.clear_first {
        for reg in [AVC_MAD, AVC_OVERTIME_MB, AVC_ME_INFO] {
            ops.push(write(reg, 0));
        }
    }
    // Clear what an earlier frame left in the status, then enable.
    ops.push(write(AVC_STATUS, STATUS_ENDED));
    ops.push(write(AVC_INT_EN, hypothesis.interrupts));
    ops.push(write(AVC_TRIGGER, TRIGGER_START));
    ops.push(Op::Wait);
    ops
}

fn newer_para1(stride: u32, qp: u32) -> u32 {
    PARA1_NEWER | PARA1_DYNAMIC_ME | (stride / 16).div_ceil(48) << 10 | qp
}

/// A whole frame in the order of the vendor's library for the H5 and the H6:
/// the input stage; interrupts, status and the first parameters; the
/// stream; the buffers; the headers through the bit writer; the last
/// parameters; the start and the wait.
fn frame_input_first(
    hypothesis: &Hypothesis,
    stream: &Stream,
    stride: u32,
    slice: &Slice,
    index: u32,
    buffers: &Buffers,
    header_ops: &[HeaderOp],
) -> Vec<Op> {
    let own = buffers.reconstruction[(index % 2) as usize];
    let last = if slice.key {
        own
    } else {
        buffers.reconstruction[((index + 1) % 2) as usize]
    };
    let write = |reg, value| Op::Write { reg, value };
    let block = |reg, address| Op::Write {
        reg,
        value: hypothesis.block(address),
    };
    let qp = slice.qp as u32 & 0x3f;
    let columns = stride / 16;

    let mut ops = input(hypothesis, stream, stride, buffers);
    ops.push(Op::Update {
        reg: AVC_INT_EN,
        clear: 0,
        set: hypothesis.interrupts,
    });
    // Written ones clear what an earlier frame left.
    ops.push(Op::Update {
        reg: AVC_STATUS,
        clear: 0,
        set: 0x7,
    });
    ops.push(Op::Update {
        reg: AVC_PARA1,
        clear: 0x3f | 0xf << 10,
        set: qp | PARA1_DYNAMIC_ME | columns.div_ceil(48) << 10,
    });
    // The thresholds of the dynamic motion estimation grow with the width.
    ops.push(write(
        AVC_DYNAMIC_ME0,
        ((columns * 6 / 10) << 16) | (columns / 10),
    ));
    ops.push(write(
        AVC_DYNAMIC_ME1,
        ((columns * 32 / 10) << 16) | (columns * 26 / 10),
    ));
    ops.push(write(AVC_RC_MAD_SUM, 0));
    // No rate control in the engine: every frame has the QP it is given.
    ops.push(write(AVC_RC_INIT, 0));
    ops.extend(stream_registers(hypothesis, buffers));
    ops.push(block(AVC_REF_LUMA, last.luma));
    ops.push(block(AVC_REF_CHROMA, last.chroma));
    ops.push(block(AVC_REC_LUMA, own.luma));
    ops.push(block(AVC_REC_CHROMA, own.chroma));
    ops.push(block(AVC_SUBPIXEL_LAST, last.subpixel));
    ops.push(block(AVC_SUBPIXEL_NEW, own.subpixel));
    ops.push(block(AVC_MB_INFO, buffers.macroblock_info));
    ops.push(write(AVC_DEBLOCK, 0));
    ops.push(write(AVC_INTRA_REFRESH, 0));
    ops.extend(headers(hypothesis, header_ops));

    let mut para0 = 0;
    if !slice.key {
        para0 |= PARA0_P_SLICE | PARA0_NEWER_P;
    }
    if stream.cabac {
        para0 |= PARA0_CABAC;
    }
    if !stream.deblocking {
        para0 |= PARA0_NO_DEBLOCKING;
    }
    ops.push(write(AVC_PARA0, para0));
    ops.push(write(AVC_PARA1, newer_para1(stride, qp)));
    ops.push(write(AVC_PARA2, 0));
    ops.push(write(AVC_OVERTIME_MB, 0));
    ops.push(write(AVC_MV_BUFFER, 0));
    ops.push(write(AVC_ME, hypothesis.me));
    ops.push(write(AVC_TRIGGER, TRIGGER_START));
    ops.push(Op::Wait);
    ops
}

/// Everything a frame needs, in the hypothesis' order.
pub fn frame(
    hypothesis: &Hypothesis,
    stream: &Stream,
    stride: u32,
    slice: &Slice,
    index: u32,
    buffers: &Buffers,
    header_ops: &[HeaderOp],
) -> Vec<Op> {
    if hypothesis.input_first {
        return frame_input_first(
            hypothesis, stream, stride, slice, index, buffers, header_ops,
        );
    }
    let mut ops = stream_buffer(hypothesis, buffers);
    ops.extend(headers(hypothesis, header_ops));
    ops.extend(input(hypothesis, stream, stride, buffers));
    ops.extend(encode(hypothesis, stream, stride, slice, index, buffers));
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAM: Stream = Stream {
        width: 1920,
        height: 1080,
        fps: 30,
        cabac: false,
        chroma_qp_offset: 4,
        deblocking: true,
    };

    const BUFFERS: Buffers = Buffers {
        input_luma: 0x4100_0000,
        input_chroma: 0x4120_0000,
        stream: 0x4200_0000,
        stream_bytes: 0x0010_0000,
        reconstruction: [
            Reconstruction {
                luma: 0x4300_0000,
                chroma: 0x4330_0000,
                subpixel: 0x4360_0000,
            },
            Reconstruction {
                luma: 0x4400_0000,
                chroma: 0x4430_0000,
                subpixel: 0x4460_0000,
            },
        ],
        macroblock_info: 0x4500_0000,
        extra: 0x4600_0000,
    };

    fn writes(ops: &[Op]) -> Vec<(u32, u32)> {
        ops.iter()
            .map(|op| match *op {
                Op::Write { reg, value } => (reg, value),
                other => panic!("not a write: {other:?}"),
            })
            .collect()
    }

    #[test]
    fn buffer_sizes_are_those_of_the_sources() {
        // The numbers of the design note's table for 1920x1088.
        let older = Sizes::older(&STREAM);
        assert_eq!(older.input, 3_133_440);
        assert_eq!(older.reconstruction_luma, 2_088_960);
        assert_eq!(older.reconstruction_chroma, 1_044_480);
        assert_eq!(older.subpixel, 522_240);
        assert_eq!(older.macroblock_info, 3_840);
        assert_eq!(older.extra, 65_280);
        let v3s = Sizes::v3s(&STREAM);
        assert_eq!(v3s.reconstruction_luma, 2_211_840);
        assert_eq!(v3s.reconstruction_chroma, 1_105_920);
        assert_eq!(v3s.subpixel, 352 * 145);
        assert_eq!(v3s.macroblock_info, 16_384);
        let lab = Sizes::lab(&STREAM);
        assert_eq!(lab.reconstruction_luma, 2_211_840);
        assert_eq!(lab.subpixel, 522_240);
        assert_eq!(lab.extra, 65_280);
    }

    #[test]
    fn the_older_map_selects_engine_eleven() {
        assert_eq!(
            select(&Hypothesis::OLDER),
            [Op::Write {
                reg: MODE,
                value: 0x0013_000b
            }]
        );
        assert_eq!(
            deselect(&Hypothesis::OLDER),
            [Op::Write {
                reg: MODE,
                value: 0x0013_0007
            }]
        );
    }

    #[test]
    fn the_newer_map_resets_the_encoder_and_sets_its_bits() {
        assert_eq!(
            select(&Hypothesis::V3S),
            [
                Op::Update {
                    reg: MODE,
                    clear: 0xcf,
                    set: 7
                },
                Op::Update {
                    reg: RESET,
                    clear: 0,
                    set: 1 << 24
                },
                Op::Update {
                    reg: RESET,
                    clear: 1 << 24,
                    set: 0
                },
                Op::Update {
                    reg: MODE,
                    clear: 0,
                    set: 0xc7
                },
            ]
        );
    }

    #[test]
    fn the_stream_buffer_is_bounded_before_anything_is_written() {
        assert_eq!(
            writes(&stream_buffer(&Hypothesis::OLDER, &BUFFERS)),
            [
                (0xb88, 0),
                (0xb80, 0x4200_0000),
                (0xb84, 0x420f_ffff),
                (0xb8c, 0x0080_0000),
            ]
        );
        assert_eq!(
            writes(&stream_buffer(&Hypothesis::V3S, &BUFFERS)),
            [
                (0xb18, 0),
                (0xb88, 0),
                (0xb80, 0x4200_0000),
                (0xb84, 0x420f_ffff),
                (0xb8c, 0x0080_0000),
                (0xb90, 0),
                (0xb58, 0),
                (0xb54, 0),
            ]
        );
    }

    #[test]
    fn the_input_is_the_picture_in_macroblocks() {
        assert_eq!(
            writes(&input(&Hypothesis::OLDER, &STREAM, 1920, &BUFFERS)),
            [
                (0xa00, 120 << 16 | 68),
                (0xa04, 120 << 16),
                (0xa08, 0),
                (0xa78, 0x4100_0000),
                (0xa7c, 0x4120_0000),
            ]
        );
        assert_eq!(
            writes(&input(&Hypothesis::V3S, &STREAM, 1920, &BUFFERS)),
            [
                (0xa00, 120 << 16 | 68),
                (0xa2c, 68 << 16 | 120),
                (0xa04, 120 << 16),
                (0xa08, 0),
                (0xa78, 0x4100_0000),
                (0xa7c, 0x4120_0000),
            ]
        );
    }

    fn slice(key: bool) -> Slice {
        Slice {
            key,
            frame_num: if key { 0 } else { 1 },
            idr_pic_id: 0,
            qp: 30,
            cabac_init_idc: 0,
        }
    }

    /// A plan's writes, and that it ends with the wait.
    fn frame(hypothesis: &Hypothesis, key: bool, index: u32) -> Vec<(u32, u32)> {
        let ops = encode(hypothesis, &STREAM, 1920, &slice(key), index, &BUFFERS);
        assert_eq!(ops.last(), Some(&Op::Wait));
        writes(&ops[..ops.len() - 1])
    }

    #[test]
    fn a_p_frame_by_the_older_map() {
        // After jemk's h264enc_encode_picture: reconstruction, reference,
        // the two buffers of unknown purpose, the parameters, the trigger.
        assert_eq!(
            frame(&Hypothesis::OLDER, false, 1),
            [
                (0xbc0, 0x4500_0000),
                (0xb60, 0x4600_0000),
                (0xbb0, 0x4400_0000),
                (0xbb4, 0x4430_0000),
                (0xba0, 0x4300_0000),
                (0xba4, 0x4330_0000),
                (0xbbc, 0x4460_0000),
                (0xbb8, 0x4360_0000),
                (0xb04, 0x10),
                (0xb08, 4 << 16 | 30 << 8 | 30),
                (0xb10, 0x104),
                (0xb1c, 0xf),
                (0xb14, 0xf),
                (0xb18, 8),
            ]
        );
    }

    #[test]
    fn a_key_frame_by_the_older_map_names_no_reference() {
        assert_eq!(
            frame(&Hypothesis::OLDER, true, 0),
            [
                (0xbc0, 0x4500_0000),
                (0xb60, 0x4600_0000),
                (0xbb0, 0x4300_0000),
                (0xbb4, 0x4330_0000),
                (0xbbc, 0x4360_0000),
                (0xb04, 0),
                (0xb08, 4 << 16 | 30 << 8 | 30),
                (0xb10, 0x104),
                (0xb1c, 0xf),
                (0xb14, 0xf),
                (0xb18, 8),
            ]
        );
    }

    #[test]
    fn a_p_frame_by_the_newer_map() {
        // After Bootlin's cedrus_enc_h264_job_configure and job_trigger. A
        // stride of 120 macroblocks is 3 times 48.
        assert_eq!(
            frame(&Hypothesis::V3S, false, 1),
            [
                (0xbc0, 0x4500_0000),
                (0xb60, 0),
                (0xbb0, 0x4400_0000),
                (0xbb4, 0x4430_0000),
                (0xba0, 0x4300_0000),
                (0xba4, 0x4330_0000),
                (0xbbc, 0x4460_0000),
                (0xbb8, 0x4360_0000),
                (0xbc4, 0),
                (0xb28, 0),
                (0xb04, 0x10),
                (0xb08, 4 << 16 | 3 << 10 | 30),
                (0xb0c, 0),
                (0xb48, 0),
                (0xb4c, 0),
                (0xb2c, 0),
                (0xb30, 0),
                (0xb34, 0),
                (0xb38, 0),
                (0xb3c, 0),
                (0xb10, 0x0001_0004),
                (0xb50, 0),
                (0xb24, 0),
                (0xb5c, 0),
                (0xb1c, 0xf),
                (0xb14, 3),
                (0xb18, 8),
            ]
        );
    }

    #[test]
    fn a_key_frame_by_the_newer_map_references_itself() {
        let writes = frame(&Hypothesis::V3S, true, 0);
        assert_eq!(
            writes[2..8],
            [
                (0xbb0, 0x4300_0000),
                (0xbb4, 0x4330_0000),
                (0xba0, 0x4300_0000),
                (0xba4, 0x4330_0000),
                (0xbbc, 0x4360_0000),
                (0xbb8, 0x4360_0000),
            ]
        );
        assert!(writes.contains(&(0xb04, 0)));
    }

    #[test]
    fn cabac_and_no_deblocking_reach_the_parameters() {
        let stream = Stream {
            cabac: true,
            deblocking: false,
            ..STREAM
        };
        let slice = Slice {
            cabac_init_idc: 1,
            ..slice(false)
        };
        let ops = encode(&Hypothesis::V3S, &stream, 1920, &slice, 1, &BUFFERS);
        assert!(ops.contains(&Op::Write {
            reg: AVC_PARA0,
            value: 0x10 | 0x100 | 1 << 10 | 1 << 12
        }));
        let ops = encode(&Hypothesis::OLDER, &stream, 1920, &slice, 1, &BUFFERS);
        assert!(ops.contains(&Op::Write {
            reg: AVC_PARA0,
            value: 0x110
        }));
    }

    #[test]
    fn headers_go_through_the_bit_writer() {
        let header_ops = [
            HeaderOp::Start(0x67),
            HeaderOp::Bits(Put::new(0x0001_2345, 20)),
        ];
        // The older map: no waits. Three puts for the start code and the
        // header byte between the two changes of the prevention bit, then
        // 20 bits in two pieces.
        assert_eq!(
            headers(&Hypothesis::OLDER, &header_ops),
            [
                Op::Update {
                    reg: 0xb04,
                    clear: 0,
                    set: 1 << 31
                },
                Op::Write {
                    reg: 0xb20,
                    value: 0
                },
                Op::Write {
                    reg: 0xb18,
                    value: 1 | 16 << 8
                },
                Op::Write {
                    reg: 0xb20,
                    value: 1
                },
                Op::Write {
                    reg: 0xb18,
                    value: 1 | 16 << 8
                },
                Op::Write {
                    reg: 0xb20,
                    value: 0x67
                },
                Op::Write {
                    reg: 0xb18,
                    value: 1 | 8 << 8
                },
                Op::Update {
                    reg: 0xb04,
                    clear: 1 << 31,
                    set: 0
                },
                Op::Write {
                    reg: 0xb20,
                    value: 0x1234
                },
                Op::Write {
                    reg: 0xb18,
                    value: 1 | 16 << 8
                },
                Op::Write {
                    reg: 0xb20,
                    value: 5
                },
                Op::Write {
                    reg: 0xb18,
                    value: 1 | 4 << 8
                },
            ]
        );
        // The newer map waits for the writer before each of the five puts
        // and for the idle bits at the end.
        let newer = headers(&Hypothesis::V3S, &header_ops);
        let ready = Op::Until {
            reg: 0xb1c,
            mask: 1 << 9,
        };
        assert_eq!(newer.iter().filter(|op| **op == ready).count(), 5);
        assert_eq!(
            newer.last(),
            Some(&Op::Until {
                reg: 0x004,
                mask: 0x300
            })
        );
    }

    /// The buffers of the vendor library's run for the H6 behind the trace
    /// (320x240), at its made-up addresses.
    const TRACED: Buffers = Buffers {
        input_luma: 0x6b84_d000,
        input_chroma: 0x6b86_1000,
        stream: 0x6b00_0000,
        stream_bytes: 0x0080_0000,
        reconstruction: [
            Reconstruction {
                luma: 0x6b80_1000,
                chroma: 0x6b81_5000,
                subpixel: 0x6b83_f000,
            },
            Reconstruction {
                luma: 0x6b82_0000,
                chroma: 0x6b83_4000,
                subpixel: 0x6b84_1000,
            },
        ],
        macroblock_info: 0x6b84_3000,
        extra: 0,
    };

    fn small() -> Stream {
        Stream {
            width: 320,
            height: 240,
            chroma_qp_offset: 0,
            ..STREAM
        }
    }

    /// A frame by the H6 hypothesis without its headers' puts: the writes,
    /// and the three registers that are read and written back.
    fn traced_frame(key: bool, index: u32) -> (Vec<(u32, u32)>, Vec<Op>) {
        let ops = super::frame(
            &Hypothesis::H6,
            &small(),
            320,
            &slice(key),
            index,
            &TRACED,
            &[],
        );
        assert_eq!(ops.last(), Some(&Op::Wait));
        let updates = ops
            .iter()
            .copied()
            .filter(|op| matches!(op, Op::Update { .. }))
            .collect();
        let written = ops
            .iter()
            .filter_map(|op| match *op {
                Op::Write { reg, value } => Some((reg, value)),
                _ => None,
            })
            .collect();
        (written, updates)
    }

    #[test]
    fn a_key_frame_as_the_vendors_library_writes_it_for_the_h6() {
        // Register for register what the library wrote behind the trace:
        // the picture in units of 8 pixels, the input's addresses as they
        // are, the encoder block's in units of 256 bytes.
        let (written, updates) = traced_frame(true, 0);
        assert_eq!(
            written,
            [
                (0xa00, 0x0028_001e),
                (0xa04, 0x0014_0000),
                (0xa14, 0x0000_0014),
                (0xa2c, 0x001e_0028),
                (0xa08, 0),
                (0xa78, 0x6b84_d000),
                (0xa7c, 0x6b86_1000),
                (0xa80, 0x6b86_5b00),
                (0xb48, 0x000c_0002),
                (0xb4c, 0x0040_0034),
                (0xb40, 0),
                (0xb2c, 0),
                (0xb88, 0),
                (0xb80, 0x006b_0000),
                (0xb84, 0x006b_7fff),
                (0xb8c, 0x0400_0000),
                (0xba0, 0x006b_8010),
                (0xba4, 0x006b_8150),
                (0xbb0, 0x006b_8010),
                (0xbb4, 0x006b_8150),
                (0xbb8, 0x006b_83f0),
                (0xbbc, 0x006b_83f0),
                (0xbc0, 0x006b_8430),
                (0xbc4, 0),
                (0xb28, 0),
                (0xb04, 0),
                (0xb08, 0x0800_051e),
                (0xb0c, 0),
                (0xb24, 0),
                (0xb60, 0),
                (0xb10, 0x0001_1000),
                (0xb18, 8),
            ]
        );
        // It read 0 and wrote 0xf; read 0x260 and wrote 0x267; read 0x1e
        // and wrote 0x51e.
        assert_eq!(
            updates,
            [
                Op::Update {
                    reg: 0xb14,
                    clear: 0,
                    set: 0xf
                },
                Op::Update {
                    reg: 0xb1c,
                    clear: 0,
                    set: 7
                },
                Op::Update {
                    reg: 0xb08,
                    clear: 0x3c3f,
                    set: 0x51e
                },
            ]
        );
    }

    #[test]
    fn a_p_frame_as_the_vendors_library_writes_it_for_the_h6() {
        let (written, _) = traced_frame(false, 1);
        // The reference is the frame before; the two sets take turns.
        assert_eq!(
            written[16..22],
            [
                (0xba0, 0x006b_8010),
                (0xba4, 0x006b_8150),
                (0xbb0, 0x006b_8200),
                (0xbb4, 0x006b_8340),
                (0xbb8, 0x006b_83f0),
                (0xbbc, 0x006b_8410),
            ]
        );
        assert!(written.contains(&(0xb04, 0x410)));
        let stream = Stream {
            cabac: true,
            ..small()
        };
        let ops = super::frame(
            &Hypothesis::H6,
            &stream,
            320,
            &slice(false),
            1,
            &TRACED,
            &[],
        );
        assert!(ops.contains(&Op::Write {
            reg: AVC_PARA0,
            value: 0x510
        }));
    }

    #[test]
    fn a_search_is_one_bit_of_the_motion_estimation_register() {
        let plain = super::frame(
            &Hypothesis::H6,
            &small(),
            320,
            &slice(false),
            1,
            &TRACED,
            &[],
        );
        // The vendor's plan is the exact search, this board's map the wide.
        assert_eq!(
            adjusted(plain.clone(), &[search_adjust(Search::Exact)]),
            plain
        );
        let wide = adjusted(plain.clone(), &[search_adjust(Search::Wide)]);
        assert_eq!(
            wide,
            super::frame(
                &Hypothesis::H616,
                &small(),
                320,
                &slice(false),
                1,
                &TRACED,
                &[]
            )
        );
        assert_eq!(adjusted(wide, &[search_adjust(Search::Exact)]), plain);
        assert_eq!(Search::Exact.other(), Search::Wide);
        assert_eq!(Search::Wide.other().name(), "exact");
        let adjust = search_adjust(Search::Wide);
        assert_eq!(
            Adjust::new(adjust.reg, adjust.clear, adjust.set),
            Some(adjust)
        );
    }

    #[test]
    fn this_boards_plan_is_the_h6s_with_one_bit_more() {
        assert_eq!(Hypothesis::named("h616"), Some(Hypothesis::H616));
        for key in [true, false] {
            let frame = |hypothesis: &Hypothesis| {
                super::frame(hypothesis, &small(), 320, &slice(key), 1, &TRACED, &[])
            };
            let (h6, h616) = (frame(&Hypothesis::H6), frame(&Hypothesis::H616));
            let different: Vec<_> = h6.iter().zip(&h616).filter(|(a, b)| a != b).collect();
            assert_eq!(
                different,
                [(
                    &Op::Write {
                        reg: AVC_ME,
                        value: 0x0001_1000
                    },
                    &Op::Write {
                        reg: AVC_ME,
                        value: 0x0001_1010
                    }
                )]
            );
            assert_eq!(h6.len(), h616.len());
        }
        assert_eq!(select(&Hypothesis::H616), select(&Hypothesis::H6));
        // The bit is one a lab run may change.
        assert!(Adjust::new(AVC_ME, 0, ME_REFINE_OFF).is_some());
    }

    #[test]
    fn a_lab_run_changes_only_fields_that_search_and_decide() {
        // Where and whether the engine writes is not a lab run's to change.
        for bit in [10, 16, 17, 18, 19, 22, 23, 31] {
            assert_eq!(Adjust::new(AVC_ME, 0, 1 << bit), None, "bit {bit}");
            assert_eq!(Adjust::new(AVC_ME, 1 << bit, 0), None, "bit {bit}");
        }
        for reg in [
            AVC_REF_LUMA,
            AVC_STREAM_START,
            AVC_MV_BUFFER,
            AVC_PARA0,
            MODE,
        ] {
            assert_eq!(Adjust::new(reg, 0, 1), None);
            assert_eq!(Adjust::new(reg, 1, 0), None);
        }
        assert_eq!(Adjust::new(AVC_PARA1, 0, PARA1_NEWER), None);
        assert_eq!(Adjust::new(AVC_PARA1, 0x3f, 0), None);

        let plain = super::frame(
            &Hypothesis::H6,
            &small(),
            320,
            &slice(false),
            1,
            &TRACED,
            &[],
        );
        let adjusts = [
            Adjust::new(AVC_ME, 0, 0x4).unwrap(),
            Adjust::new(AVC_PARA1, PARA1_DYNAMIC_ME, 1 << 26).unwrap(),
            Adjust::new(AVC_DYNAMIC_ME0, 0x03ff_03ff, 0).unwrap(),
        ];
        let changed = adjusted(plain.clone(), &adjusts);
        assert_eq!(changed.len(), plain.len());
        let mut differences = Vec::new();
        for (before, after) in plain.iter().zip(&changed) {
            if before != after {
                differences.push((*before, *after));
            }
        }
        let qp = slice(false).qp as u32;
        assert_eq!(
            differences,
            [
                (
                    Op::Update {
                        reg: AVC_PARA1,
                        clear: 0x3f | 0xf << 10,
                        set: qp | PARA1_DYNAMIC_ME | 1 << 10,
                    },
                    Op::Update {
                        reg: AVC_PARA1,
                        clear: 0x3f | 0xf << 10 | PARA1_DYNAMIC_ME,
                        set: qp | 1 << 10 | 1 << 26,
                    },
                ),
                (
                    Op::Write {
                        reg: AVC_DYNAMIC_ME0,
                        value: 0x000c_0002,
                    },
                    Op::Write {
                        reg: AVC_DYNAMIC_ME0,
                        value: 0,
                    },
                ),
                (
                    Op::Write {
                        reg: AVC_PARA1,
                        value: PARA1_NEWER | PARA1_DYNAMIC_ME | 1 << 10 | qp,
                    },
                    Op::Write {
                        reg: AVC_PARA1,
                        value: PARA1_NEWER | 1 << 26 | 1 << 10 | qp,
                    },
                ),
                (
                    Op::Write {
                        reg: AVC_ME,
                        value: 0x0001_1000,
                    },
                    Op::Write {
                        reg: AVC_ME,
                        value: 0x0001_1004,
                    },
                ),
            ]
        );
        assert_eq!(adjusted(plain.clone(), &[]), plain);
    }

    #[test]
    fn the_sizes_the_vendors_library_wrote_for_larger_pictures() {
        let values = |width: u32, height: u32, stride: u32| {
            let stream = Stream {
                width,
                height,
                chroma_qp_offset: 0,
                ..STREAM
            };
            let ops = super::frame(
                &Hypothesis::H6,
                &stream,
                stride,
                &slice(true),
                0,
                &TRACED,
                &[],
            );
            let value = |wanted: u32| {
                ops.iter()
                    .rev()
                    .find_map(|op| match *op {
                        Op::Write { reg, value } if reg == wanted => Some(value),
                        _ => None,
                    })
                    .unwrap()
            };
            [
                value(0xa00),
                value(0xa04),
                value(0xa14),
                value(0xa2c),
                value(0xb08),
                value(0xb48),
                value(0xb4c),
            ]
        };
        assert_eq!(
            values(1920, 1080, 1920),
            [
                0x00f0_0087,
                0x0078_0000,
                0x78,
                0x0087_00f0,
                0x0800_0d1e,
                0x0048_000c,
                0x0180_0138
            ]
        );
        assert_eq!(
            values(1366, 768, 1376),
            [
                0x00ab_0060,
                0x0056_0000,
                0x56,
                0x0060_00ab,
                0x0800_091e,
                0x0033_0008,
                0x0113_00df
            ]
        );
        assert_eq!(
            values(1280, 720, 1280)[..5],
            [0x00a0_005a, 0x0050_0000, 0x50, 0x005a_00a0, 0x0800_091e]
        );
    }

    #[test]
    fn headers_alone_end_with_the_align_trigger_on_the_newer_generation() {
        assert_eq!(
            writes(&stream_buffer(&Hypothesis::H6, &TRACED)),
            [
                (0xb18, 0),
                (0xb88, 0),
                (0xb80, 0x006b_0000),
                (0xb84, 0x006b_7fff),
                (0xb8c, 0x0400_0000),
            ]
        );
        assert_eq!(writes(&headers_end(&Hypothesis::H6)), [(0xb18, 2)]);
        assert!(headers_end(&Hypothesis::V3S).is_empty());
        // No wait for the idle bits after the headers: the library reads
        // the reset register nowhere.
        let ops = headers(&Hypothesis::H6, &[HeaderOp::Start(0x67)]);
        assert!(
            !ops.iter()
                .any(|op| matches!(op, Op::Until { reg: 0x004, .. }))
        );
    }

    #[test]
    fn the_select_can_set_the_memory_interface_field() {
        assert_eq!(
            select(&Hypothesis::H6).last(),
            Some(&Op::Update {
                reg: MODE,
                clear: 0x0013_0000,
                set: 0x0002_00c7
            })
        );
        assert_eq!(
            select(&Hypothesis::H6_DECODER_MODE).last(),
            Some(&Op::Update {
                reg: MODE,
                clear: 0x0013_0000,
                set: 0x0013_00c7
            })
        );
        assert_eq!(select(&Hypothesis::H6_PLAIN_MODE), select(&Hypothesis::V3S));
        assert_eq!(Hypothesis::named("h6"), Some(Hypothesis::H6));
        assert_eq!(Hypothesis::H6.chroma_qp_offset(), 0);
        assert_eq!(Hypothesis::V3S.chroma_qp_offset(), 4);
    }

    #[test]
    fn the_mask_probe_names_no_trigger_and_no_status() {
        for reg in MASK_PROBE {
            assert!(
                ![
                    AVC_TRIGGER,
                    AVC_STATUS,
                    AVC_INT_EN,
                    AVC_PUT_DATA,
                    MODE,
                    RESET
                ]
                .contains(&reg)
            );
            assert!((0xa00..0xc00).contains(&reg));
            assert!(!name(reg).is_empty(), "{reg:#x}");
        }
        let mut sorted = MASK_PROBE.to_vec();
        sorted.dedup();
        assert_eq!(sorted.len(), MASK_PROBE.len());
    }
}
