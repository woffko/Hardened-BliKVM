/* SPDX-License-Identifier: GPL-3.0-only */
#define _GNU_SOURCE
#include <fcntl.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include "harness.h"
#include "log.h"
#include "window.h"

#define RECORDS (4u << 20)
#define ARENA_ALIGN 4096u
#define ARENA_BUFFERS 256

static int trace_fd = -1;

static struct {
	uint8_t *memory;
	size_t bytes;
	size_t used;
	uint64_t base;
	struct {
		size_t offset;
		size_t bytes;
		int live;
	} buffers[ARENA_BUFFERS];
	int count;
} arena;

static void say(const char *text)
{
	ssize_t ignored = write(2, text, strlen(text));

	(void)ignored;
}

void harness_fatal(const char *why)
{
	say("ve-trace: ");
	say(why);
	say("\n");
	log_add(REC_NOTE, why, 0, 0, 0);
	if (trace_fd >= 0)
		log_dump(trace_fd);
	_exit(70);
}

int harness_start(const struct model_config *config, const char *trace_path)
{
	static const struct window_hooks hooks = {
		.write = model_write,
		.read = model_read,
		.fatal = harness_fatal,
	};

	if (trace_path) {
		trace_fd = open(trace_path, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
		if (trace_fd < 0)
			return 0;
	}
	if (!log_reserve(RECORDS) || !window_open(MODEL_WINDOW, &hooks))
		return 0;
	model_start(window_back(), config);
	window_sync();
	return 1;
}

int harness_finish(void)
{
	if (log_overflowed())
		say("ve-trace: the log was full; its end is missing\n");
	if (trace_fd < 0)
		return 1;
	return log_dump(trace_fd) && close(trace_fd) == 0;
}

void engine_select(const char *name, int encoder)
{
	log_add(REC_OP, name, (uint64_t)encoder, 0, 0);
	model_select(encoder);
	window_sync();
}

void engine_reset(const char *name)
{
	log_add(REC_OP, name, 0, 0, 0);
	model_reset();
	window_sync();
}

int engine_wait(const char *name)
{
	int finished = model_wait();

	log_add(REC_OP, name, (uint64_t)finished, 0, 0);
	window_sync();
	return finished;
}

void engine_memory_mode(const char *name, unsigned int width, int reconstruction_mode)
{
	log_add(REC_OP, name, width, (uint64_t)reconstruction_mode, 0);
	model_memory_mode(width, reconstruction_mode);
	window_sync();
}

int arena_start(size_t bytes, uint64_t base)
{
	void *memory = mmap(NULL, bytes, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);

	if (memory == MAP_FAILED)
		return 0;
	memset(&arena, 0, sizeof(arena));
	arena.memory = memory;
	arena.bytes = bytes;
	arena.base = base;
	return 1;
}

void *arena_allocate(size_t bytes)
{
	size_t rounded = (bytes + ARENA_ALIGN - 1) & ~(size_t)(ARENA_ALIGN - 1);
	int number = arena.count;

	/* A page between two buffers, so that an end is not the next one's start. */
	if (!bytes || number == ARENA_BUFFERS || arena.used + rounded + ARENA_ALIGN > arena.bytes) {
		log_add(REC_NOTE, "arena: no room", bytes, 0, 0);
		return NULL;
	}
	arena.buffers[number].offset = arena.used;
	arena.buffers[number].bytes = bytes;
	arena.buffers[number].live = 1;
	arena.count++;
	arena.used += rounded + ARENA_ALIGN;
	log_add(REC_ALLOC, NULL, (uint64_t)number, bytes, arena.base + arena.buffers[number].offset);
	return arena.memory + arena.buffers[number].offset;
}

int arena_buffer(const void *memory)
{
	const uint8_t *at = memory;
	int number;

	if (at < arena.memory || at >= arena.memory + arena.bytes)
		return -1;
	for (number = 0; number < arena.count; number++) {
		size_t offset = (size_t)(at - arena.memory);

		if (offset >= arena.buffers[number].offset &&
		    offset < arena.buffers[number].offset + arena.buffers[number].bytes)
			return number;
	}
	return -1;
}

void arena_free(void *memory)
{
	int number = arena_buffer(memory);

	log_add(REC_FREE, NULL, (uint64_t)(int64_t)number, 0, 0);
	if (number >= 0)
		arena.buffers[number].live = 0;
}

uint64_t arena_address(const void *memory)
{
	const uint8_t *at = memory;

	if (at < arena.memory || at >= arena.memory + arena.bytes)
		return 0;
	return arena.base + (uint64_t)(at - arena.memory);
}

void *arena_pointer(uint64_t address)
{
	if (address < arena.base || address >= arena.base + arena.bytes)
		return NULL;
	return arena.memory + (address - arena.base);
}
