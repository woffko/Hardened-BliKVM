/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * How one instruction is let through, by architecture:
 *
 * - AArch64: a breakpoint (`brk #0`) is put in the place of the next
 *   instruction and taken away when it is hit. A load or store never
 *   branches, so the next instruction is the one four bytes on. The
 *   handler keeps the word its breakpoint replaces and puts it back; it
 *   does not look at it.
 * - x86-64 (the development host, for this project's own test): the
 *   processor's single-step flag in the interrupted context.
 *
 * Whether an access is a load or a store is learnt without looking at the
 * instruction either: the window is opened for reading first, and an
 * instruction that faults a second time is a store.
 */
#define _GNU_SOURCE
#include <signal.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <ucontext.h>
#include <unistd.h>

#include "log.h"
#include "window.h"

#if !defined(__aarch64__) && !defined(__x86_64__)
#error "the window knows AArch64 and x86-64"
#endif

enum state { CLOSED, OPEN_READ, OPEN_WRITE };

static struct {
	uint8_t *guest;
	uint32_t *back;
	uint32_t *shadow;
	size_t bytes;
	size_t page;
	struct window_hooks hooks;
	enum state state;
	uintptr_t pc;
	uintptr_t address;
	uint64_t accesses;
	struct {
		uint32_t index;
		uint32_t raw;
	} *changed;
#if defined(__aarch64__)
	uint32_t *breakpoint;
	uint32_t replaced;
#endif
} w;

static void fatal(const char *why)
{
	w.hooks.fatal(why);
	_exit(70);
}

static uintptr_t context_pc(const ucontext_t *context)
{
#if defined(__aarch64__)
	return (uintptr_t)context->uc_mcontext.pc;
#else
	return (uintptr_t)context->uc_mcontext.gregs[REG_RIP];
#endif
}

#if defined(__aarch64__)
#define BREAKPOINT 0xd4200000u

static void put_word(uint32_t *where, uint32_t word)
{
	void *page = (void *)((uintptr_t)where & ~(w.page - 1));

	/* The page stays executable: the code that runs now may live in it. */
	if (mprotect(page, w.page, PROT_READ | PROT_WRITE | PROT_EXEC) != 0)
		fatal("a code page cannot be made writable");
	*where = word;
	__builtin___clear_cache((char *)where, (char *)(where + 1));
	if (mprotect(page, w.page, PROT_READ | PROT_EXEC) != 0)
		fatal("a code page cannot be closed again");
}

static void step_arm(ucontext_t *context)
{
	w.breakpoint = (uint32_t *)(context_pc(context) + 4);
	w.replaced = *w.breakpoint;
	put_word(w.breakpoint, BREAKPOINT);
}

static int step_disarm(ucontext_t *context)
{
	if (context_pc(context) != (uintptr_t)w.breakpoint)
		return 0;
	put_word(w.breakpoint, w.replaced);
	w.breakpoint = NULL;
	return 1;
}
#else
#define TRAP_FLAG 0x100

static void step_arm(ucontext_t *context)
{
	context->uc_mcontext.gregs[REG_EFL] |= TRAP_FLAG;
}

static int step_disarm(ucontext_t *context)
{
	context->uc_mcontext.gregs[REG_EFL] &= ~(greg_t)TRAP_FLAG;
	return 1;
}
#endif

static void protect(int protection)
{
	if (mprotect(w.guest, w.bytes, protection) != 0)
		fatal("the window's protection cannot be changed");
}

static void on_fault(int signal_number, siginfo_t *info, void *opaque)
{
	ucontext_t *context = opaque;
	uintptr_t address = (uintptr_t)info->si_addr;
	uintptr_t pc = context_pc(context);

	(void)signal_number;
	if (address < (uintptr_t)w.guest || address >= (uintptr_t)w.guest + w.bytes)
		fatal("a fault outside the window: the traced code crashed");
	if (w.state == CLOSED) {
		w.state = OPEN_READ;
		w.pc = pc;
		w.address = address;
		protect(PROT_READ);
		step_arm(context);
	} else if (w.state == OPEN_READ && pc == w.pc) {
		/* The same instruction again: it stores. */
		w.state = OPEN_WRITE;
		w.address = address;
		protect(PROT_READ | PROT_WRITE);
	} else {
		fatal("a fault while the window was open");
	}
}

static void on_trap(int signal_number, siginfo_t *info, void *opaque)
{
	ucontext_t *context = opaque;
	uint32_t offset = (uint32_t)(w.address - (uintptr_t)w.guest) & ~3u;
	size_t words = w.bytes / 4;
	size_t index;
	size_t changed = 0;

	(void)signal_number;
	(void)info;
	if (w.state == CLOSED || !step_disarm(context))
		fatal("a trap that is not the window's");
	protect(PROT_NONE);
	w.accesses++;
	if (w.state == OPEN_READ) {
		uint32_t value = w.shadow[offset / 4];

		log_add(REC_READ, NULL, offset, value, 0);
		w.state = CLOSED;
		if (w.hooks.read)
			w.hooks.read(offset, value);
	} else {
		/*
		 * Every word the instruction changed, in ascending order; a
		 * store that changed nothing wrote the word it faulted at.
		 * The words are taken down before the model hears of the
		 * first: its answer to one may change others.
		 */
		for (index = 0; index < words; index++) {
			if (w.back[index] == w.shadow[index])
				continue;
			w.changed[changed].index = (uint32_t)index;
			w.changed[changed].raw = w.back[index];
			changed++;
		}
		if (!changed) {
			w.changed[0].index = offset / 4;
			w.changed[0].raw = w.back[offset / 4];
			changed = 1;
		}
		w.state = CLOSED;
		for (index = 0; index < changed; index++) {
			uint32_t at = w.changed[index].index;
			uint32_t raw = w.changed[index].raw;
			uint32_t old = w.shadow[at];

			log_add(REC_WRITE, NULL, (uint64_t)at * 4, raw, old);
			w.back[at] = w.hooks.write(at * 4, raw, old);
		}
	}
	memcpy(w.shadow, w.back, w.bytes);
}

int window_open(size_t bytes, const struct window_hooks *hooks)
{
	struct sigaction action;
	int fd;

	w.page = (size_t)sysconf(_SC_PAGESIZE);
	if (!bytes || bytes % w.page || !hooks || !hooks->write || !hooks->fatal)
		return 0;
	fd = (int)syscall(SYS_memfd_create, "ve-trace-window", 0);
	if (fd < 0 || ftruncate(fd, (off_t)bytes) != 0)
		return 0;
	w.guest = mmap(NULL, bytes, PROT_NONE, MAP_SHARED, fd, 0);
	w.back = mmap(NULL, bytes, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	w.shadow = mmap(NULL, bytes, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	w.changed = mmap(NULL, bytes / 4 * sizeof(*w.changed), PROT_READ | PROT_WRITE,
			 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	close(fd);
	if (w.guest == MAP_FAILED || (void *)w.back == MAP_FAILED || (void *)w.shadow == MAP_FAILED ||
	    (void *)w.changed == MAP_FAILED)
		return 0;
	w.bytes = bytes;
	w.hooks = *hooks;
	w.state = CLOSED;
	w.accesses = 0;

	memset(&action, 0, sizeof(action));
	action.sa_flags = SA_SIGINFO;
	sigemptyset(&action.sa_mask);
	/* Neither handler may be interrupted by the other. */
	sigaddset(&action.sa_mask, SIGSEGV);
	sigaddset(&action.sa_mask, SIGTRAP);
	action.sa_sigaction = on_fault;
	if (sigaction(SIGSEGV, &action, NULL) != 0)
		return 0;
	action.sa_sigaction = on_trap;
	if (sigaction(SIGTRAP, &action, NULL) != 0)
		return 0;
	return 1;
}

void *window_guest(void)
{
	return w.guest;
}

uint32_t *window_back(void)
{
	return w.back;
}

size_t window_bytes(void)
{
	return w.bytes;
}

void window_sync(void)
{
	memcpy(w.shadow, w.back, w.bytes);
}

uint64_t window_accesses(void)
{
	return w.accesses;
}
