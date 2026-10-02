/* SPDX-License-Identifier: GPL-3.0-only */
#ifndef VE_TRACE_SELFTEST_CLIENT_H
#define VE_TRACE_SELFTEST_CLIENT_H

#include <stdint.h>

#define SELFTEST_LOOP 1000

struct selftest_keeper {
	void (*select)(int encoder);
	int (*wait)(void);
};

/* Touches the registers behind `window` in the ways selftest.c expects. */
uint64_t selftest_client(void *window, const struct selftest_keeper *keeper);

#endif
