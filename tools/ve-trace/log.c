/* SPDX-License-Identifier: GPL-3.0-only */
#include <errno.h>
#include <sys/mman.h>
#include <unistd.h>

#include "log.h"

_Static_assert(sizeof(struct record) == 64, "the report tool reads 64-byte records");

static struct record *records;
static size_t capacity;
static size_t count;
static int overflowed;

int log_reserve(size_t wanted)
{
	void *memory = mmap(NULL, wanted * sizeof(struct record), PROT_READ | PROT_WRITE,
			    MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);

	if (memory == MAP_FAILED)
		return 0;
	records = memory;
	capacity = wanted;
	count = 0;
	overflowed = 0;
	return 1;
}

void log_add(enum record_kind kind, const char *name, uint64_t a, uint64_t b, uint64_t c)
{
	struct record *record;
	size_t index = 0;

	if (count == capacity) {
		overflowed = 1;
		return;
	}
	record = &records[count++];
	record->kind = (uint8_t)kind;
	for (; name && name[index] && index < RECORD_NAME - 1; index++)
		record->name[index] = name[index];
	for (; index < RECORD_NAME; index++)
		record->name[index] = 0;
	record->a = a;
	record->b = b;
	record->c = c;
}

size_t log_count(void)
{
	return count;
}

const struct record *log_at(size_t index)
{
	return index < count ? &records[index] : NULL;
}

int log_overflowed(void)
{
	return overflowed;
}

int log_dump(int fd)
{
	const char *at = (const char *)records;
	size_t left = count * sizeof(struct record);

	while (left) {
		ssize_t done = write(fd, at, left);

		if (done < 0 && errno == EINTR)
			continue;
		if (done <= 0)
			return 0;
		at += done;
		left -= (size_t)done;
	}
	return 1;
}
