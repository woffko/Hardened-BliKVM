/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * The encoder driver's checks and its register plan, as plain functions of
 * their arguments: no kernel service, no hardware. The driver carries the
 * plan out; a program on the build host tests it with random requests
 * (kernel/hardened-blikvm-ve/test).
 */
#ifndef HARDENED_BLIKVM_VE_ENC_PLAN_H
#define HARDENED_BLIKVM_VE_ENC_PLAN_H

#include <linux/types.h>

#include "ve_enc_abi.h"

/* Every buffer is a whole number of these, at a multiple of them. */
#define VE_ENC_PAGE			4096u
/* The most one configuration may allocate. */
#define VE_ENC_MAX_TOTAL_BYTES		(96u << 20)

/* A stream's sizes, all derived from its width, height and flags. */
struct ve_enc_geometry {
	u32 width;
	u32 height;
	u32 flags;
	/* Macroblocks. */
	u32 columns;
	u32 rows;
	/* What a picture is: columns by rows of macroblocks, NV12. */
	u32 picture_bytes;
	/* The buffers, each a whole number of pages. */
	u32 input_bytes;
	u32 stream_bytes;
	u32 rec_luma_bytes;
	u32 rec_bytes;
	u32 subpixel_bytes;
	u32 mb_info_bytes;
};

/*
 * Sets of reconstruction buffers: the reference, and two for passes over
 * the picture that is next, so that of two passes either can be the next
 * picture's reference.
 */
#define VE_ENC_SETS			3

/* Where the buffers are, as the engine addresses them. */
struct ve_enc_addresses {
	u32 input;
	u32 stream;
	u32 rec[VE_ENC_SETS];
	u32 subpixel[VE_ENC_SETS];
	u32 mb_info;
};

/* No set. */
#define VE_ENC_NONE			(-1)

/* Which set holds what; VE_ENC_NONE where there is none. */
struct ve_enc_sets {
	/*
	 * The frame the picture of the last pass refers to: the frame that
	 * was sent before it. A key frame has one too, although the engine
	 * does not read it: a key frame that is not sent leaves the stream
	 * where it was.
	 */
	int reference;
	/* What the last pass wrote. */
	int last;
	/* What the pass before it wrote, if it was over the same picture. */
	int earlier;
};

/* The sets of one pass. */
struct ve_enc_pass {
	/* Written by the pass. */
	u32 own;
	/* Read by the engine: the picture's reference, of a key frame its own. */
	u32 reference;
	/* The picture's reference as the sets keep it. */
	int kept;
};

/* What of a request reaches the plan. */
struct ve_enc_request {
	u32 flags;
	u32 qp;
	u32 unit_count;
	const struct ve_enc_unit *units;
};

enum ve_enc_op_kind {
	/* The register is written `value`. */
	VE_ENC_OP_WRITE,
	/* The register is read, `clear` is cleared, `value` set, written back. */
	VE_ENC_OP_UPDATE,
	/* The low `bits` bits of `value` go through the bit writer. */
	VE_ENC_OP_PUT,
	/* The encode is started; the plan's last operation. */
	VE_ENC_OP_START,
};

struct ve_enc_op {
	u16 reg;
	u8 kind;
	u8 bits;
	u32 value;
	u32 clear;
};

/* Selecting the encoder, the frame's registers, three units' bits, the start. */
#define VE_ENC_MAX_OPS			320

/* The registers the plan may name. */
#define VE_ENC_REG_MODE			0x000
#define VE_ENC_REG_RESET		0x004
#define VE_ENC_REG_STATUS		0xb1c
#define VE_ENC_REG_PUT_DATA		0xb20
#define VE_ENC_REG_TRIGGER		0xb18
#define VE_ENC_REG_LENGTH		0xb90
#define VE_ENC_TRIGGER_PUT		1u
#define VE_ENC_TRIGGER_START		8u
#define VE_ENC_STATUS_PUT_READY		(1u << 9)

/* 0, or -EINVAL for a size or a flag the interface does not have. */
int ve_enc_geometry(u32 width, u32 height, u32 flags,
		    struct ve_enc_geometry *geometry);

/* The bytes all buffers of a configuration take. */
u64 ve_enc_total_bytes(const struct ve_enc_geometry *geometry);

/* The sets of a stream that has no frame yet, or lost its last one. */
void ve_enc_sets_reset(struct ve_enc_sets *sets);

/*
 * 0 and the two sets of the pass a request asks for, or -EINVAL: a flag, a
 * QP or a unit out of range; flags that contradict each other; a picture
 * to be encoded again while there is none (`have_picture`); a frame that
 * is not a key frame while there is nothing to refer to, or that names a
 * pass that was not made. `sets` is not changed.
 */
int ve_enc_check_frame(const struct ve_enc_request *request,
		       const struct ve_enc_sets *sets, int have_picture,
		       struct ve_enc_pass *pass);

/* The pass was made: what the sets hold now. */
void ve_enc_sets_done(struct ve_enc_sets *sets, u32 flags,
		      const struct ve_enc_pass *pass);

/*
 * The operations of a pass. Returns their number, or 0 if `ops` is too
 * short.
 */
unsigned int ve_enc_plan(const struct ve_enc_geometry *geometry,
			 const struct ve_enc_addresses *addresses,
			 const struct ve_enc_request *request,
			 const struct ve_enc_pass *pass,
			 struct ve_enc_op *ops, unsigned int room);

#endif
