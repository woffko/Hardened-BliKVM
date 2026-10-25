/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * The lab module's device, /dev/hardened-blikvm-ve-lab: what a lab tool asks
 * of it. One client at a time, root only.
 *
 * Mappings: offset 0 is the engine's register window; offset
 * (index + 1) << VE_LAB_BUFFER_SHIFT is buffer `index`, in its whole length.
 */
#ifndef HARDENED_BLIKVM_VE_LAB_ABI_H
#define HARDENED_BLIKVM_VE_LAB_ABI_H

#include <linux/ioctl.h>
#include <linux/types.h>

#define VE_LAB_ABI			1
#define VE_LAB_MAX_BUFFERS		16
#define VE_LAB_MAX_BUFFER_BYTES		(16u << 20)
#define VE_LAB_MAX_TOTAL_BYTES		(96u << 20)
#define VE_LAB_BUFFER_SHIFT		28
#define VE_LAB_MAX_WAIT_MS		10000

struct ve_lab_info {
	__u32 abi;
	/* The engine's version register as read after power-on. */
	__u32 version;
	__u32 window_bytes;
	__u32 max_buffers;
};

struct ve_lab_alloc {
	/* In: the bytes wanted; out: the bytes given (whole pages). */
	__u32 bytes;
	__u32 index;
	/* The address the engine reaches the buffer at. */
	__u32 address;
	__u32 reserved;
	/* The offset to map it at. */
	__u64 offset;
};

struct ve_lab_wait {
	__u32 timeout_ms;
	/* The encoder's interrupt came; `status` is what it reported. */
	__u32 came;
	__u32 status;
	/* An interrupt came that was not the encoder's; the line is off. */
	__u32 unknown;
};

#define VE_LAB_IOC_INFO		_IOR('V', 0x10, struct ve_lab_info)
#define VE_LAB_IOC_ALLOC	_IOWR('V', 0x11, struct ve_lab_alloc)
#define VE_LAB_IOC_WAIT		_IOWR('V', 0x12, struct ve_lab_wait)
#define VE_LAB_IOC_RESET	_IO('V', 0x13)

#endif
