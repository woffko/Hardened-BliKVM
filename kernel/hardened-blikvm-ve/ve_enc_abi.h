/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * The encoder driver's device, /dev/hardened-blikvm-ve-enc: what its client
 * asks of it (docs/design/h616-ve-encoder-driver-v1.md). One client at a
 * time. Nothing here is an address of the engine's or a register value.
 */
#ifndef HARDENED_BLIKVM_VE_ENC_ABI_H
#define HARDENED_BLIKVM_VE_ENC_ABI_H

#include <linux/ioctl.h>
#include <linux/types.h>

#define VE_ENC_ABI			1
/*
 * The visible picture: even numbers within these. The engine does not
 * finish a frame that refers to another when the picture is narrower than
 * 13 macroblocks (docs/design/h616-ve-h264-encoder.md, "V3.1 again"); the
 * driver takes none below 16.
 */
#define VE_ENC_MIN_WIDTH		256
#define VE_ENC_MIN_HEIGHT		64
#define VE_ENC_MAX_WIDTH		1920
#define VE_ENC_MAX_HEIGHT		1088
/* A frame's header units: SPS, PPS, slice header. */
#define VE_ENC_MAX_UNITS		3
#define VE_ENC_MAX_UNIT_BYTES		128
#define VE_ENC_MAX_QP			51

/* Flags of a configuration. */
#define VE_ENC_CABAC			(1u << 0)
#define VE_ENC_CONFIG_FLAGS		VE_ENC_CABAC

/*
 * Flags of a frame. Every request is one pass of the engine over a picture.
 * The frame a picture refers to is, of the passes over the picture before
 * it, the last one, unless the request says otherwise.
 */
#define VE_ENC_KEY			(1u << 0)
/* The last picture once more, against the same reference. */
#define VE_ENC_AGAIN			(1u << 1)
/*
 * The engine's other search for motion: it finds a picture that moves down,
 * and text that moves by a few pixels costs more with it.
 */
#define VE_ENC_WIDE			(1u << 2)
/* A new picture refers to the pass before the last one. */
#define VE_ENC_EARLIER			(1u << 3)
/*
 * A new picture refers to what the picture before it referred to: no pass
 * over that picture was sent.
 */
#define VE_ENC_STAY			(1u << 4)
#define VE_ENC_FRAME_FLAGS		(VE_ENC_KEY | VE_ENC_AGAIN | VE_ENC_WIDE | \
					 VE_ENC_EARLIER | VE_ENC_STAY)

struct ve_enc_info {
	__u32 abi;
	/* The engine's two identification registers. */
	__u32 decoder_ip;
	__u32 encoder_ip;
	__u32 max_width;
	__u32 max_height;
	__u32 min_width;
	__u32 min_height;
	__u32 reserved;
};

struct ve_enc_config {
	__u32 width;
	__u32 height;
	__u32 flags;
	__u32 reserved;
	/* Out: the bytes of a picture, and the most a frame can have. */
	__u32 picture_bytes;
	__u32 stream_bytes;
};

/*
 * One unit of a frame's headers: the byte after the start code, and the
 * payload's bits, the most significant bit of the first byte first.
 */
struct ve_enc_unit {
	__u8 header;
	__u8 reserved;
	__u16 bits;
	__u8 payload[VE_ENC_MAX_UNIT_BYTES];
};

struct ve_enc_frame {
	__u32 flags;
	__u32 qp;
	/* NV12 of whole macroblocks; not read with VE_ENC_AGAIN. */
	__u64 picture;
	/* struct ve_enc_unit[unit_count] */
	__u64 units;
	/* Where the frame goes. */
	__u64 stream;
	__u32 picture_bytes;
	__u32 unit_count;
	__u32 stream_capacity;
	/* Out. With ENOSPC: the bytes the frame has. */
	__u32 stream_bytes;
	/* The encoder's status bits as the interrupt found them. */
	__u32 status;
	__u32 encode_us;
	/* The engine's numbers about the frame: 0xb50, 0xb54, 0xb58. */
	__u32 statistics[3];
	__u32 reserved;
};

#define VE_ENC_IOC_INFO		_IOR('V', 0x20, struct ve_enc_info)
#define VE_ENC_IOC_CONFIGURE	_IOWR('V', 0x21, struct ve_enc_config)
#define VE_ENC_IOC_ENCODE	_IOWR('V', 0x22, struct ve_enc_frame)

#endif
