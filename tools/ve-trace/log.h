/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * The trace: a list of fixed-size records in memory that was set aside
 * before the run, so that a signal handler can add to it. It is written
 * out as it is; `ve_trace_report.py` reads it.
 */
#ifndef VE_TRACE_LOG_H
#define VE_TRACE_LOG_H

#include <stddef.h>
#include <stdint.h>

enum record_kind {
	REC_READ = 1,   /* a: offset, b: value */
	REC_WRITE = 2,  /* a: offset, b: value as written, c: value before */
	REC_CALL = 3,   /* name: a function of the library, called by us */
	REC_RETURN = 4, /* name; a: what it returned */
	REC_OP = 5,     /* name: a function of ours, called by the library; a, b, c */
	REC_ALLOC = 6,  /* a: number, b: bytes, c: the address the engine is told */
	REC_FREE = 7,   /* a: number */
	REC_NOTE = 8,   /* name: something the model saw; a, b, c */
};

#define RECORD_NAME 39

struct record {
	uint8_t kind;
	char name[RECORD_NAME];
	uint64_t a, b, c;
};

/* Room for this many records; false if there is no memory. */
int log_reserve(size_t records);
/* Safe in a signal handler. A full log drops the record and remembers. */
void log_add(enum record_kind kind, const char *name, uint64_t a, uint64_t b, uint64_t c);
size_t log_count(void);
const struct record *log_at(size_t index);
int log_overflowed(void);
/* The records as they are, to a file descriptor; safe in a signal handler. */
int log_dump(int fd);

#endif
