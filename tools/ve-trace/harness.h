/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * What every run of the trace needs: the log, the closed window with the
 * engine's stand-in behind it, the engine's keeper (what a kernel driver
 * and its user-space half do for a client), and memory that plays the
 * engine's buffers.
 */
#ifndef VE_TRACE_HARNESS_H
#define VE_TRACE_HARNESS_H

#include <stddef.h>
#include <stdint.h>

#include "model.h"

/* The trace goes to `trace_path` at the end, and on a fatal error. */
int harness_start(const struct model_config *config, const char *trace_path);
void harness_fatal(const char *why) __attribute__((noreturn));
/* Writes the trace; 0 for failure. */
int harness_finish(void);

/* The keeper's parts; each is recorded as an operation of ours. */
void engine_select(const char *name, int encoder);
void engine_reset(const char *name);
int engine_wait(const char *name);
void engine_memory_mode(const char *name, unsigned int width, int reconstruction_mode);

/*
 * Buffers. Memory of this process, each with a made-up address for the
 * engine: `base` plus its place in the arena. Nothing lives at these
 * addresses; they only have to be told apart in what the library writes.
 */
int arena_start(size_t bytes, uint64_t base);
void *arena_allocate(size_t bytes);
void arena_free(void *memory);
/* The made-up address of a pointer into the arena; 0 if it is not one. */
uint64_t arena_address(const void *memory);
/* The pointer for a made-up address; NULL if it is not one. */
void *arena_pointer(uint64_t address);
/* The number of the buffer a pointer lies in, -1 if none. */
int arena_buffer(const void *memory);

#endif
