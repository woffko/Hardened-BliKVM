// SPDX-License-Identifier: GPL-2.0-only
/*
 * The register plan of a frame on the H616's H.264 encoder: what the
 * vendor's library writes for the H6's encoder, of which this engine's is
 * the next revision, as this project traced and then confirmed it on the
 * board (docs/design/h616-ve-h264-encoder.md, "The H616's register
 * description"). The lab tool's hypothesis `h6` is the same plan in Rust;
 * the test on the build host compares the two operation by operation.
 */
#include <linux/errno.h>
#include <linux/types.h>

#include "ve_enc_plan.h"

#define MODE_DECODER_FIELD	0xfu
#define MODE_DECODER_NONE	7u
#define MODE_INPUT_STAGE	(1u << 6)
#define MODE_ENCODER		(1u << 7)
#define MODE_MEMORY_FIELD	0x00130000u
/* What the vendor's keeper sets on the H6. */
#define MODE_MEMORY		(2u << 16)
#define RESET_ENCODER		(1u << 24)

#define ISP_SIZE		0xa00
#define ISP_STRIDE		0xa04
#define ISP_CTRL		0xa08
#define ISP_STRIDE_MBS		0xa14
#define ISP_SCALER_SIZE		0xa2c
#define ISP_INPUT_LUMA		0xa78
#define ISP_INPUT_CHROMA	0xa7c
#define ISP_INPUT_CHROMA2	0xa80
#define AVC_PARA0		0xb04
#define AVC_PARA1		0xb08
#define AVC_PARA2		0xb0c
#define AVC_ME			0xb10
#define AVC_INT_EN		0xb14
#define AVC_OVERTIME_MB		0xb24
#define AVC_INTRA_REFRESH	0xb28
#define AVC_RC_INIT		0xb2c
#define AVC_RC_MAD_SUM		0xb40
#define AVC_DYNAMIC_ME0		0xb48
#define AVC_DYNAMIC_ME1		0xb4c
#define AVC_MV_BUFFER		0xb60
#define AVC_STREAM_START	0xb80
#define AVC_STREAM_END		0xb84
#define AVC_STREAM_OFFSET	0xb88
#define AVC_STREAM_MAX		0xb8c
#define AVC_REF_LUMA		0xba0
#define AVC_REF_CHROMA		0xba4
#define AVC_REC_LUMA		0xbb0
#define AVC_REC_CHROMA		0xbb4
#define AVC_SUBPIXEL_LAST	0xbb8
#define AVC_SUBPIXEL_NEW	0xbbc
#define AVC_MB_INFO		0xbc0
#define AVC_DEBLOCK		0xbc4

#define PARA0_P_SLICE		(1u << 4)
#define PARA0_CABAC		(1u << 8)
#define PARA0_NEWER_P		(1u << 10)
#define PARA0_NO_PREVENTION	(1u << 31)
#define PARA1_DYNAMIC_ME	(1u << 8)
#define PARA1_NEWER		(1u << 27)
/* The vendor's value, and the bit of the other search ("refine off"). */
#define ME_VALUE		0x00011000u
#define ME_WIDE			(1u << 4)
#define INT_ENABLES		0xfu
#define STATUS_CLEAR		0x7u

/* The bit writer is given at most this many bits at a time. */
#define PUT_BITS		16u

static u32 round_up_to(u32 value, u32 to)
{
	return (value + to - 1) / to * to;
}

int ve_enc_geometry(u32 width, u32 height, u32 flags,
		    struct ve_enc_geometry *geometry)
{
	u32 columns, rows, subpixel_width, subpixel_height;

	if (width < VE_ENC_MIN_WIDTH || width > VE_ENC_MAX_WIDTH ||
	    height < VE_ENC_MIN_HEIGHT || height > VE_ENC_MAX_HEIGHT ||
	    (width & 1) || (height & 1) || (flags & ~VE_ENC_CONFIG_FLAGS))
		return -EINVAL;

	columns = (width + 15) / 16;
	rows = (height + 15) / 16;
	geometry->width = width;
	geometry->height = height;
	geometry->flags = flags;
	geometry->columns = columns;
	geometry->rows = rows;
	/* At most 120 by 68 macroblocks: nothing here comes near 32 bits. */
	geometry->picture_bytes = columns * 16 * rows * 16 * 3 / 2;
	geometry->input_bytes = round_up_to(geometry->picture_bytes, VE_ENC_PAGE);
	/* Room for a frame that is as large as its picture. */
	geometry->stream_bytes = round_up_to(geometry->picture_bytes, 1u << 20);
	/* The sizes of the V3s map (Bootlin's driver), which the vendor's
	 * library asks its allocator for on the H6 as well. */
	geometry->rec_luma_bytes = round_up_to(columns, 2) * 16 *
				   round_up_to(rows + 1, 4) * 16;
	geometry->rec_bytes = round_up_to(geometry->rec_luma_bytes +
		round_up_to(columns, 2) * 16 *
		round_up_to((rows + 1) / 2, 4) * 16, VE_ENC_PAGE) + VE_ENC_PAGE;
	subpixel_width = (columns + 47) * 2 / 3 / 32 * 32 +
			 round_up_to(columns, 32) * 2;
	subpixel_height = (rows * 16 + 72) / 8;
	geometry->subpixel_bytes = round_up_to(subpixel_width * subpixel_height,
					       VE_ENC_PAGE) + VE_ENC_PAGE;
	geometry->mb_info_bytes = (columns + 31) / 32 * 4096 + VE_ENC_PAGE;
	return 0;
}

u64 ve_enc_total_bytes(const struct ve_enc_geometry *geometry)
{
	return (u64)geometry->input_bytes + geometry->stream_bytes +
	       VE_ENC_SETS * (u64)geometry->rec_bytes +
	       VE_ENC_SETS * (u64)geometry->subpixel_bytes +
	       geometry->mb_info_bytes;
}

void ve_enc_sets_reset(struct ve_enc_sets *sets)
{
	sets->reference = VE_ENC_NONE;
	sets->last = VE_ENC_NONE;
	sets->earlier = VE_ENC_NONE;
}

/* A set that is neither of the two. */
static u32 another(int first, int second)
{
	u32 set;

	for (set = 0; set < VE_ENC_SETS - 1; set++) {
		if ((int)set != first && (int)set != second)
			break;
	}
	return set;
}

int ve_enc_check_frame(const struct ve_enc_request *request,
		       const struct ve_enc_sets *sets, int have_picture,
		       struct ve_enc_pass *pass)
{
	u32 flags = request->flags;
	int reference = sets->reference;
	unsigned int index;

	if ((flags & ~VE_ENC_FRAME_FLAGS) ||
	    request->qp > VE_ENC_MAX_QP ||
	    request->unit_count < 1 || request->unit_count > VE_ENC_MAX_UNITS)
		return -EINVAL;
	for (index = 0; index < request->unit_count; index++) {
		if (request->units[index].bits > 8 * VE_ENC_MAX_UNIT_BYTES)
			return -EINVAL;
	}

	if (flags & VE_ENC_AGAIN) {
		/* The same picture against the same reference. */
		if ((flags & (VE_ENC_EARLIER | VE_ENC_STAY)) || !have_picture ||
		    sets->last == VE_ENC_NONE)
			return -EINVAL;
		/* The last pass stays; the one before it is written over. */
		pass->own = another(reference, sets->last);
	} else {
		if ((flags & VE_ENC_EARLIER) && (flags & VE_ENC_STAY))
			return -EINVAL;
		if (flags & VE_ENC_EARLIER) {
			if (sets->earlier == VE_ENC_NONE)
				return -EINVAL;
			reference = sets->earlier;
		} else if (!(flags & VE_ENC_STAY) && sets->last != VE_ENC_NONE) {
			reference = sets->last;
		}
		pass->own = another(reference, VE_ENC_NONE);
	}
	pass->kept = reference;
	if (flags & VE_ENC_KEY) {
		/* A key frame names its own set as its reference. */
		pass->reference = pass->own;
		return 0;
	}
	if (reference == VE_ENC_NONE)
		return -EINVAL;
	pass->reference = reference;
	return 0;
}

void ve_enc_sets_done(struct ve_enc_sets *sets, u32 flags,
		      const struct ve_enc_pass *pass)
{
	sets->earlier = (flags & VE_ENC_AGAIN) ? sets->last : VE_ENC_NONE;
	sets->last = pass->own;
	sets->reference = pass->kept;
}

struct plan {
	struct ve_enc_op *ops;
	unsigned int count;
	unsigned int room;
};

static void add(struct plan *plan, u8 kind, u16 reg, u32 value, u32 clear,
		u8 bits)
{
	if (plan->count < plan->room) {
		struct ve_enc_op *op = &plan->ops[plan->count];

		op->kind = kind;
		op->reg = reg;
		op->value = value;
		op->clear = clear;
		op->bits = bits;
	}
	/* Counted even when it does not fit, so that the caller learns it. */
	plan->count++;
}

static void write(struct plan *plan, u16 reg, u32 value)
{
	add(plan, VE_ENC_OP_WRITE, reg, value, 0, 0);
}

static void update(struct plan *plan, u16 reg, u32 clear, u32 set)
{
	add(plan, VE_ENC_OP_UPDATE, reg, set, clear, 0);
}

static void put(struct plan *plan, u32 value, u8 bits)
{
	add(plan, VE_ENC_OP_PUT, VE_ENC_REG_PUT_DATA, value, 0, bits);
}

/* An address as an address register of the encoder block holds it. */
static u32 block(u32 address)
{
	return address >> 8;
}

/*
 * One unit through the bit writer: the start code and the header byte
 * without emulation prevention, the payload with it.
 */
static void unit(struct plan *plan, const struct ve_enc_unit *unit)
{
	unsigned int done = 0;

	update(plan, AVC_PARA0, 0, PARA0_NO_PREVENTION);
	put(plan, 0, 16);
	put(plan, 1, 16);
	put(plan, unit->header, 8);
	update(plan, AVC_PARA0, PARA0_NO_PREVENTION, 0);
	while (done < unit->bits) {
		unsigned int take = unit->bits - done;
		unsigned int bit;
		u32 value = 0;

		if (take > PUT_BITS)
			take = PUT_BITS;
		for (bit = done; bit < done + take; bit++)
			value = (value << 1) |
				((unit->payload[bit / 8] >> (7 - bit % 8)) & 1);
		put(plan, value, take);
		done += take;
	}
}

unsigned int ve_enc_plan(const struct ve_enc_geometry *geometry,
			 const struct ve_enc_addresses *addresses,
			 const struct ve_enc_request *request,
			 const struct ve_enc_pass *pass,
			 struct ve_enc_op *ops, unsigned int room)
{
	struct plan plan = { .ops = ops, .count = 0, .room = room };
	int key = request->flags & VE_ENC_KEY;
	u32 own = pass->own;
	u32 last = pass->reference;
	u32 columns = geometry->columns;
	u32 stride = columns * 16;
	u32 width8 = (geometry->width + 7) / 8;
	u32 height8 = (geometry->height + 7) / 8;
	u32 chroma = addresses->input + stride * geometry->rows * 16;
	u32 para1 = request->qp | PARA1_DYNAMIC_ME | ((columns + 47) / 48) << 10;
	u32 para0 = 0;
	unsigned int at;

	/* The encoder, after a pulse of its reset. */
	update(&plan, VE_ENC_REG_MODE,
	       MODE_ENCODER | MODE_INPUT_STAGE | MODE_DECODER_FIELD,
	       MODE_DECODER_NONE);
	update(&plan, VE_ENC_REG_RESET, 0, RESET_ENCODER);
	update(&plan, VE_ENC_REG_RESET, RESET_ENCODER, 0);
	update(&plan, VE_ENC_REG_MODE, MODE_MEMORY_FIELD,
	       MODE_MEMORY | MODE_ENCODER | MODE_INPUT_STAGE | MODE_DECODER_NONE);

	/* The input stage: the visible size in units of 8 pixels. */
	write(&plan, ISP_SIZE, width8 << 16 | height8);
	write(&plan, ISP_STRIDE, columns << 16);
	write(&plan, ISP_STRIDE_MBS, columns);
	write(&plan, ISP_SCALER_SIZE, height8 << 16 | width8);
	write(&plan, ISP_CTRL, 0);
	write(&plan, ISP_INPUT_LUMA, addresses->input);
	write(&plan, ISP_INPUT_CHROMA, chroma);
	write(&plan, ISP_INPUT_CHROMA2, chroma + stride * geometry->height / 4);

	update(&plan, AVC_INT_EN, 0, INT_ENABLES);
	/* Written ones clear what an earlier frame left. */
	update(&plan, VE_ENC_REG_STATUS, 0, STATUS_CLEAR);
	update(&plan, AVC_PARA1, 0x3f | 0xf << 10, para1);
	write(&plan, AVC_DYNAMIC_ME0, (columns * 6 / 10) << 16 | columns / 10);
	write(&plan, AVC_DYNAMIC_ME1,
	      (columns * 32 / 10) << 16 | columns * 26 / 10);
	write(&plan, AVC_RC_MAD_SUM, 0);
	write(&plan, AVC_RC_INIT, 0);

	/* The stream: exactly its buffer. */
	write(&plan, AVC_STREAM_OFFSET, 0);
	write(&plan, AVC_STREAM_START, block(addresses->stream));
	write(&plan, AVC_STREAM_END,
	      block(addresses->stream + geometry->stream_bytes - 1));
	write(&plan, AVC_STREAM_MAX, geometry->stream_bytes * 8);

	write(&plan, AVC_REF_LUMA, block(addresses->rec[last]));
	write(&plan, AVC_REF_CHROMA,
	      block(addresses->rec[last] + geometry->rec_luma_bytes));
	write(&plan, AVC_REC_LUMA, block(addresses->rec[own]));
	write(&plan, AVC_REC_CHROMA,
	      block(addresses->rec[own] + geometry->rec_luma_bytes));
	write(&plan, AVC_SUBPIXEL_LAST, block(addresses->subpixel[last]));
	write(&plan, AVC_SUBPIXEL_NEW, block(addresses->subpixel[own]));
	write(&plan, AVC_MB_INFO, block(addresses->mb_info));
	write(&plan, AVC_DEBLOCK, 0);
	write(&plan, AVC_INTRA_REFRESH, 0);

	for (at = 0; at < request->unit_count; at++)
		unit(&plan, &request->units[at]);

	if (!key)
		para0 |= PARA0_P_SLICE | PARA0_NEWER_P;
	if (geometry->flags & VE_ENC_CABAC)
		para0 |= PARA0_CABAC;
	write(&plan, AVC_PARA0, para0);
	write(&plan, AVC_PARA1, PARA1_NEWER | para1);
	write(&plan, AVC_PARA2, 0);
	write(&plan, AVC_OVERTIME_MB, 0);
	write(&plan, AVC_MV_BUFFER, 0);
	write(&plan, AVC_ME,
	      ME_VALUE | ((request->flags & VE_ENC_WIDE) ? ME_WIDE : 0));
	add(&plan, VE_ENC_OP_START, VE_ENC_REG_TRIGGER, VE_ENC_TRIGGER_START, 0, 0);

	return plan.count <= room ? plan.count : 0;
}
