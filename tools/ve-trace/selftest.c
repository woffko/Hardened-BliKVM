/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * The trace's own proof: selftest_client.c runs behind the closed window,
 * and every access it makes must come out of the trace, complete and in
 * order, with the values the engine's stand-in gives.
 *
 * usage: ve-trace-selftest CLIENT_LIBRARY [TRACE_FILE]
 */
#include <dlfcn.h>
#include <stdio.h>
#include <string.h>

#include "harness.h"
#include "log.h"
#include "selftest_client.h"
#include "window.h"

struct expected {
	uint8_t kind;
	uint32_t offset;
	uint32_t value;
	uint32_t old;
	/* Records of one group may come in any order (one copy's stores). */
	int group;
};

static struct expected expected[4096];
static size_t expected_count;

static void expect(uint8_t kind, uint32_t offset, uint32_t value, uint32_t old, int group)
{
	expected[expected_count++] = (struct expected){ kind, offset, value, old, group };
}

static void r(uint32_t offset, uint32_t value)
{
	expect(REC_READ, offset, value, 0, 0);
}

static void w(uint32_t offset, uint32_t value, uint32_t old)
{
	expect(REC_WRITE, offset, value, old, 0);
}

static void keeper_select(int encoder)
{
	engine_select("selftest: select", encoder);
}

static int keeper_wait(void)
{
	return engine_wait("selftest: wait");
}

static void build_expectation(void)
{
	uint32_t index;

	r(0x0e4, 0x00012011);
	r(0x000, 0xc0000007);
	w(0xb80, 0x12345678, 0);
	r(0xb80, 0);
	/* selected */
	r(0xb08, 0x0000001e);
	r(0xb1c, 0x00000260);
	w(0xb80, 0xffffffff, 0);
	r(0xb80, 0xfffffffc);
	w(0xb0c, 0x55, 0);
	w(0xb0c, 0x55, 0x55);
#if defined(__aarch64__)
	r(0x000, 0xc00000c7);
	w(0x000, 0xc00000c1, 0xc00000c7);
#endif
	w(0xb0c, 0xaa55, 0x55);
	w(0xb30, 0x33334444, 0);
	w(0xb34, 0x11112222, 0);
	r(0xb30, 0x33334444);
	w(0xb04, 0x80000000, 0);
	w(0xb20, 0x67, 0);
	w(0xb18, 0x801, 0);
	r(0xb90, 8);
	w(0xb18, 8, 0x801);
	r(0xb1c, 0x261);
	w(0xb1c, 0x261, 0x261);
	r(0xb1c, 0x260);
	r(0xb90, 8 + MODEL_H616.frame_bits);
	w(0xb18, 8, 8);
	r(0xb1c, 0x260);
	r(0xb1c, 0x260);
	r(0xb1c, 0x260);
	r(0xb1c, 0x261);
	for (index = 0; index < SELFTEST_LOOP; index++) {
		w(0xb0c, index, index ? index - 1 : 0xaa55);
		r(0xb0c, index);
	}
	expect(REC_WRITE, 0xb30, 0x04030201, 0x33334444, 1);
	expect(REC_WRITE, 0xb34, 0x14131211, 0x11112222, 1);
	expect(REC_WRITE, 0xb38, 0x24232221, 0, 1);
	expect(REC_WRITE, 0xb3c, 0x34333231, 0, 1);
	/* The fourth of them holds no bit on this engine. */
	r(0xb3c, 0);
	r(0x030, 0x00000200);
	/* deselected */
	r(0xb08, 0);
}

static int matches(const struct record *record, const struct expected *wanted)
{
	return record->kind == wanted->kind && record->a == wanted->offset &&
	       record->b == wanted->value && (wanted->kind == REC_READ || record->c == wanted->old);
}

int main(int argc, char **argv)
{
	uint64_t (*client)(void *, const struct selftest_keeper *);
	const struct selftest_keeper keeper = { keeper_select, keeper_wait };
	const uint8_t *stream;
	size_t stream_bytes;
	size_t index, at = 0;
	void *library;

	if (argc != 2 && argc != 3) {
		fprintf(stderr, "usage: %s CLIENT_LIBRARY [TRACE_FILE]\n", argv[0]);
		return 2;
	}
	if (!harness_start(&MODEL_H616, argc == 3 ? argv[2] : NULL)) {
		fprintf(stderr, "the window cannot be set up\n");
		return 1;
	}
	library = dlopen(argv[1], RTLD_NOW);
	client = library ? (uint64_t (*)(void *, const struct selftest_keeper *))dlsym(library, "selftest_client") : NULL;
	if (!client) {
		fprintf(stderr, "%s: %s\n", argv[1], dlerror());
		return 1;
	}
	client(window_guest(), &keeper);
	stream = model_stream(&stream_bytes);

	build_expectation();
	for (index = 0; index < log_count(); index++) {
		const struct record *record = log_at(index);
		const struct expected *wanted = &expected[at];
		size_t other;

		if (record->kind != REC_READ && record->kind != REC_WRITE)
			continue;
		if (at == expected_count) {
			fprintf(stderr, "an access too many: kind %u at %#llx\n", record->kind,
				(unsigned long long)record->a);
			return 1;
		}
		if (wanted->group) {
			/* Any member of the group that is still open. */
			for (other = at; other < expected_count && expected[other].group == wanted->group; other++)
				if (matches(record, &expected[other]))
					break;
			if (other < expected_count && expected[other].group == wanted->group) {
				struct expected found = expected[other];

				expected[other] = expected[at];
				expected[at] = found;
				at++;
				continue;
			}
		} else if (matches(record, wanted)) {
			at++;
			continue;
		}
		fprintf(stderr,
			"access %zu: kind %u at %#llx value %#llx (before: %#llx); expected kind %u at %#x value %#x (before: %#x)\n",
			at, record->kind, (unsigned long long)record->a, (unsigned long long)record->b,
			(unsigned long long)record->c, wanted->kind, wanted->offset, wanted->value, wanted->old);
		return 1;
	}
	if (at != expected_count) {
		fprintf(stderr, "the trace ends after %zu of %zu accesses\n", at, expected_count);
		return 1;
	}
	/* One store may write two registers, and the copy four. */
	if (window_accesses() < expected_count - 4 || window_accesses() > expected_count) {
		fprintf(stderr, "%llu instructions were let through for %zu accesses\n",
			(unsigned long long)window_accesses(), expected_count);
		return 1;
	}
	/* The bit writer's one byte, and nothing of the encodes. */
	if (stream_bytes != 1 || stream[0] != 0x67) {
		fprintf(stderr, "the bit writer's output is wrong (%zu bytes)\n", stream_bytes);
		return 1;
	}
	if (log_overflowed() || !harness_finish()) {
		fprintf(stderr, "the log failed\n");
		return 1;
	}
	printf("ve-trace self-test: %zu accesses came out complete and in order\n", expected_count);
	return 0;
}
