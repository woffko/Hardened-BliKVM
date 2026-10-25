/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * This project's own client of the register window, in the traced
 * library's place: a shared object that touches registers the way a driver
 * in user space does, so that the trace can be proven complete and in
 * order before anything else is traced. selftest.c holds what must come
 * out.
 */
#include <string.h>

#include "selftest_client.h"

struct mode_bits {
	uint32_t decoder : 4;
	uint32_t unused : 2;
	uint32_t input : 1;
	uint32_t encoder : 1;
	uint32_t rest : 24;
};

static uint32_t rd(volatile uint32_t *regs, unsigned int offset)
{
	return regs[offset / 4];
}

static void wr(volatile uint32_t *regs, unsigned int offset, uint32_t value)
{
	regs[offset / 4] = value;
}

/* Keeps a load and the store after it two instructions on every machine. */
#define SETTLE(value) __asm__ volatile("" : "+r"(value))

uint64_t selftest_client(void *window, const struct selftest_keeper *keeper)
{
	volatile uint32_t *regs = window;
	static const uint8_t sixteen[16] = {
		0x01, 0x02, 0x03, 0x04, 0x11, 0x12, 0x13, 0x14,
		0x21, 0x22, 0x23, 0x24, 0x31, 0x32, 0x33, 0x34,
	};
	uint64_t sum = 0;
	uint32_t value;
	unsigned int index;

	/* Identification and the mode register, before anything is selected. */
	sum += rd(regs, 0x0e4);
	sum += rd(regs, 0x000);
	/* The encoder's block holds nothing while it is not selected. */
	wr(regs, 0xb80, 0x12345678);
	sum += rd(regs, 0xb80);

	keeper->select(1);
	sum += rd(regs, 0xb08);
	sum += rd(regs, 0xb1c);
	/* A register that holds only some bits. */
	wr(regs, 0xb80, 0xffffffff);
	sum += rd(regs, 0xb80);
	/* The same value twice: the second store changes nothing. */
	wr(regs, 0xb0c, 0x55);
	wr(regs, 0xb0c, 0x55);
#if defined(__aarch64__)
	/* A field of a register, as a driver with bit-field structures sets it. */
	((volatile struct mode_bits *)regs)->decoder = 1;
#endif
	/* One byte of a register. */
	((volatile uint8_t *)regs)[0xb0c + 1] = 0xaa;
	/* Two registers in one store, and in one load. */
	*(volatile uint64_t *)((volatile uint8_t *)regs + 0xb30) = 0x1111222233334444ull;
	sum += *(volatile uint64_t *)((volatile uint8_t *)regs + 0xb30);

	/* The bit writer: eight bits without emulation prevention. */
	wr(regs, 0xb04, 0x80000000);
	wr(regs, 0xb20, 0x67);
	wr(regs, 0xb18, 1 | 8 << 8);
	sum += rd(regs, 0xb90);

	/* An encode, waited for; the status written back to clear it. */
	wr(regs, 0xb18, 8);
	sum += (uint64_t)keeper->wait();
	value = rd(regs, 0xb1c);
	SETTLE(value);
	wr(regs, 0xb1c, value);
	sum += rd(regs, 0xb1c);
	sum += rd(regs, 0xb90);

	/* An encode, polled for. */
	wr(regs, 0xb18, 8);
	for (index = 0; index < 4; index++)
		sum += rd(regs, 0xb1c);

	/* Many accesses, in order. */
	for (index = 0; index < SELFTEST_LOOP; index++) {
		wr(regs, 0xb0c, index);
		sum += rd(regs, 0xb0c);
	}

	/* Bytes copied over four registers, by whatever the compiler makes of it. */
	memcpy((uint8_t *)window + 0xb30, sixteen, sizeof(sixteen));
	sum += rd(regs, 0xb3c);

	/* A register of the top level that no map names. */
	sum += rd(regs, 0x030);

	keeper->select(0);
	sum += rd(regs, 0xb08);
	return sum;
}
