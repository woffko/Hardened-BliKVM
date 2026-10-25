/* SPDX-License-Identifier: GPL-3.0-only */
#ifndef VE_TRACE_VENDOR_OPS_H
#define VE_TRACE_VENDOR_OPS_H

#include <stdint.h>

/* What our side of the library's interface answers. */
struct vendor_answers {
	/* The engine's version as the library's own keeper would tell it. */
	uint64_t ic_version;
	int chip_id;
	int dram_type;
	/* What is taken off a buffer's address before the engine gets it. */
	uint32_t address_offset;
};

/* Before the library is loaded. */
void vendor_ops_start(const struct vendor_answers *answers);
/* The tables, typed in vendor_ops.c by the library's interface headers. */
void *vendor_engine_ops(void);
void *vendor_engine_self(void);
void *vendor_memory_ops(void);

#endif
