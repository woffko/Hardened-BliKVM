/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * A register window that is closed to the code it is given to.
 *
 * The window is memory seen twice: the traced code gets a view it may not
 * touch, the model keeps one that is always open. An access by the traced
 * code faults; the handler opens the window, lets that one instruction run
 * against the window's memory, and closes it again. Then the access is
 * recorded and the model is told. No instruction is decoded or emulated.
 */
#ifndef VE_TRACE_WINDOW_H
#define VE_TRACE_WINDOW_H

#include <stddef.h>
#include <stdint.h>

struct window_hooks {
	/*
	 * A store arrived at `offset`: `raw` is what the instruction left in
	 * the word, `old` what the word read before. Returns what the word
	 * reads from now on. May change other words through window_back().
	 */
	uint32_t (*write)(uint32_t offset, uint32_t raw, uint32_t old);
	/* A load of the word at `offset` happened; it read `value`. */
	void (*read)(uint32_t offset, uint32_t value);
	/* The run cannot go on (text for the log); must not return. */
	void (*fatal)(const char *why);
};

/* `bytes` a multiple of the page size. 0 for failure. */
int window_open(size_t bytes, const struct window_hooks *hooks);
/* What the traced code is given. */
void *window_guest(void);
/* The model's view: words, always readable and writable. */
uint32_t *window_back(void);
size_t window_bytes(void);
/* The model changed words on its own, outside its write hook. */
void window_sync(void);
/* Accesses seen so far. */
uint64_t window_accesses(void);

#endif
