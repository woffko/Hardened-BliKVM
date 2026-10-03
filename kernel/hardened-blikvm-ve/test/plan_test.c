// SPDX-License-Identifier: GPL-2.0-only
/*
 * The encoder driver's checks and register plan on the build host
 * (docs/design/h616-ve-encoder-driver-v1.md, "Tests").
 *
 * usage: plan_test random COUNT [SEED]
 *            COUNT random requests, in streams of up to forty: every pass
 *            that the checks let through must hold the invariants below,
 *            and must read and write the sets of buffers that a second
 *            account of the interface, kept here, says it must
 *        plan_test dump < DESCRIPTION
 *            the operations of one pass, for the comparison with the lab
 *            tool's plan (`bli-kvm-ve-lab plan`)
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "../ve_enc_plan.h"

/* Where the board's memory is: what an address must lie in. */
#define MEMORY_BASE 0x40000000u
#define MEMORY_END 0x80000000u

static u64 state = 0x9e3779b97f4a7c15ull;

static u32 next(void)
{
	state ^= state << 13;
	state ^= state >> 7;
	state ^= state << 17;
	return (u32)(state >> 16);
}

static u32 below(u32 bound)
{
	return next() % bound;
}

/* The registers a plan may name, and the bits each holds (V1.5). */
static const struct {
	u16 reg;
	u32 mask;
} REGISTERS[] = {
	{ 0x000, 0xffffffff }, { 0x004, 0xffffffff }, { 0xa00, 0x07ff07ff },
	{ 0xa04, 0x07ff07ff }, { 0xa08, 0xf8790007 }, { 0xa14, 0x000007ff },
	{ 0xa2c, 0x07ff07ff }, { 0xa78, 0xfffffff0 }, { 0xa7c, 0xfffffff8 },
	{ 0xa80, 0xfffffff8 }, { 0xb04, 0xffffffff }, { 0xb08, 0xffffffff },
	{ 0xb0c, 0xffffffff }, { 0xb10, 0x00ffffff }, { 0xb14, 0x0000000f },
	{ 0xb18, 0x00003f0f }, { 0xb1c, 0x0000000f }, { 0xb20, 0x0000ffff },
	{ 0xb24, 0x0003f000 }, { 0xb28, 0x80ff00ff }, { 0xb2c, 0xf03fffff },
	{ 0xb40, 0xffffffff }, { 0xb48, 0x03ff03ff }, { 0xb4c, 0x03ff03ff },
	{ 0xb60, 0xfffffffc }, { 0xb80, 0xfffffffc }, { 0xb84, 0xffffffff },
	{ 0xb88, 0xffffffff }, { 0xb8c, 0xffff0000 }, { 0xba0, 0xfffffffc },
	{ 0xba4, 0xfffffffc }, { 0xbb0, 0xfffffffc }, { 0xbb4, 0xfffffffc },
	{ 0xbb8, 0xfffffffc }, { 0xbbc, 0xfffffffc }, { 0xbc0, 0xfffffffc },
	{ 0xbc4, 0xfffffffc },
};

static int mask_of(u16 reg, u32 *mask)
{
	size_t index;

	for (index = 0; index < sizeof(REGISTERS) / sizeof(REGISTERS[0]); index++) {
		if (REGISTERS[index].reg == reg) {
			*mask = REGISTERS[index].mask;
			return 1;
		}
	}
	return 0;
}

#define FAIL(...)                                  \
	do {                                       \
		fprintf(stderr, __VA_ARGS__);      \
		fprintf(stderr, "\n");             \
		return 0;                          \
	} while (0)

/* The value the plan left in a register, or `missing`. */
static u32 last_write(const struct ve_enc_op *ops, unsigned int count, u16 reg,
		      u32 missing)
{
	u32 value = missing;
	unsigned int index;

	for (index = 0; index < count; index++)
		if (ops[index].kind == VE_ENC_OP_WRITE && ops[index].reg == reg)
			value = ops[index].value;
	return value;
}

/* 1 if the plan holds every invariant. */
static int holds(const struct ve_enc_geometry *g, const struct ve_enc_addresses *a,
		 const struct ve_enc_request *request, const struct ve_enc_pass *pass,
		 const struct ve_enc_op *ops, unsigned int count)
{
	u32 own = pass->own;
	u32 last = pass->reference;
	u32 coded = g->columns * 16 * g->rows * 16;
	unsigned int at, starts = 0, puts = 0, bits = 0, expected_bits = 0;
	u32 value;

	if (count == 0 || count > VE_ENC_MAX_OPS)
		FAIL("%u operations", count);
	if (own >= VE_ENC_SETS || last >= VE_ENC_SETS)
		FAIL("sets %u and %u", own, last);
	if (!(request->flags & VE_ENC_KEY) == (own == last))
		FAIL("a frame that is its own reference, or a key frame that is not");
	for (at = 0; at < count; at++) {
		const struct ve_enc_op *op = &ops[at];
		u32 mask;

		if (!mask_of(op->reg, &mask))
			FAIL("operation %u names register %#x", at, op->reg);
		switch (op->kind) {
		case VE_ENC_OP_WRITE:
			if (op->value & ~mask)
				FAIL("%#x is written %#x: bits outside %#x", op->reg, op->value, mask);
			if (op->reg == VE_ENC_REG_TRIGGER || op->reg == VE_ENC_REG_PUT_DATA)
				FAIL("a plain write to the trigger or the bit writer");
			break;
		case VE_ENC_OP_UPDATE:
			if ((op->value & ~mask) || (op->value & op->clear & 0))
				FAIL("%#x is set %#x: bits outside %#x", op->reg, op->value, mask);
			break;
		case VE_ENC_OP_PUT:
			if (op->reg != VE_ENC_REG_PUT_DATA || op->bits < 1 || op->bits > 16 ||
			    (op->value >> op->bits))
				FAIL("a put of %u bits of %#x", op->bits, op->value);
			puts++;
			bits += op->bits;
			break;
		case VE_ENC_OP_START:
			if (op->reg != VE_ENC_REG_TRIGGER || op->value != VE_ENC_TRIGGER_START ||
			    at != count - 1)
				FAIL("a start that is not the plan's end");
			starts++;
			break;
		default:
			FAIL("operation %u is of no kind", at);
		}
	}
	if (starts != 1)
		FAIL("%u starts", starts);
	for (at = 0; at < request->unit_count; at++)
		expected_bits += 40 + request->units[at].bits;
	if (bits != expected_bits)
		FAIL("%u bits put, %u given", bits, expected_bits);

	/* The input stage reads the picture, as it is addressed. */
	if (last_write(ops, count, 0xa78, 0) != a->input ||
	    last_write(ops, count, 0xa7c, 0) != a->input + coded)
		FAIL("the input is not the input buffer");
	value = last_write(ops, count, 0xa80, 0);
	if (value < a->input + coded || value >= a->input + g->picture_bytes)
		FAIL("the second chroma plane is outside the picture");
	if (g->picture_bytes > g->input_bytes)
		FAIL("the picture is larger than its buffer");

	/* The stream is exactly its buffer, in units of 256 bytes. */
	if ((u64)last_write(ops, count, 0xb80, 0) << 8 != a->stream)
		FAIL("the stream does not begin at its buffer");
	value = last_write(ops, count, 0xb84, 0);
	if ((((u64)value + 1) << 8) != (u64)a->stream + g->stream_bytes)
		FAIL("the stream does not end with its buffer");
	if (last_write(ops, count, 0xb8c, 0) != g->stream_bytes * 8 ||
	    last_write(ops, count, 0xb88, 1) != 0)
		FAIL("the stream's size or offset is wrong");

	/* What the engine writes besides: each inside its own buffer. */
	if ((u64)last_write(ops, count, 0xbb0, 0) << 8 != a->rec[own] ||
	    (u64)last_write(ops, count, 0xbb4, 0) << 8 != (u64)a->rec[own] + g->rec_luma_bytes ||
	    (u64)last_write(ops, count, 0xbbc, 0) << 8 != a->subpixel[own] ||
	    (u64)last_write(ops, count, 0xbc0, 0) << 8 != a->mb_info)
		FAIL("a buffer the engine writes is not the frame's own");
	if ((u64)last_write(ops, count, 0xba0, 0) << 8 != a->rec[last] ||
	    (u64)last_write(ops, count, 0xba4, 0) << 8 != (u64)a->rec[last] + g->rec_luma_bytes ||
	    (u64)last_write(ops, count, 0xbb8, 0) << 8 != a->subpixel[last])
		FAIL("the reference is not the set it must be");
	if (g->rec_luma_bytes >= g->rec_bytes || g->rec_luma_bytes % 1024)
		FAIL("the chroma plane does not begin inside the reconstruction");
	/* The registers the plan leaves at zero name no memory. */
	if (last_write(ops, count, 0xb60, 1) != 0 || last_write(ops, count, 0xbc4, 1) != 0 ||
	    last_write(ops, count, 0xb2c, 1) != 0)
		FAIL("a register that must be zero is not");
	/* The QP and the frame's type arrive in their fields. */
	value = last_write(ops, count, 0xb08, 0);
	if ((value & 0x3f) != request->qp)
		FAIL("the QP is %u, not %u", value & 0x3f, request->qp);
	value = last_write(ops, count, 0xb04, ~0u);
	if (!!(value & (1u << 4)) == !!(request->flags & VE_ENC_KEY) ||
	    !!(value & (1u << 8)) != !!(g->flags & VE_ENC_CABAC) || (value & (1u << 31)))
		FAIL("the frame's type or entropy mode is wrong: %#x", value);
	/* The search is one bit; the motion vectors are never written back. */
	value = last_write(ops, count, 0xb10, 0);
	if (value != (0x00011000u | ((request->flags & VE_ENC_WIDE) ? 0x10u : 0)))
		FAIL("the motion estimation register is %#x", value);
	(void)puts;
	return 1;
}

static int sizes_hold(const struct ve_enc_geometry *g)
{
	const u32 sizes[] = { g->input_bytes, g->stream_bytes, g->rec_bytes,
			      g->subpixel_bytes, g->mb_info_bytes };
	size_t index;

	for (index = 0; index < sizeof(sizes) / sizeof(sizes[0]); index++)
		if (!sizes[index] || sizes[index] % VE_ENC_PAGE)
			FAIL("a buffer of %u bytes", sizes[index]);
	if (ve_enc_total_bytes(g) > VE_ENC_MAX_TOTAL_BYTES)
		FAIL("%llu bytes for one stream", (unsigned long long)ve_enc_total_bytes(g));
	if (g->columns > 120 || g->rows > 68)
		FAIL("%u by %u macroblocks", g->columns, g->rows);
	return 1;
}

/* Buffers one after the other from a random place in memory. */
static void place(const struct ve_enc_geometry *g, struct ve_enc_addresses *a)
{
	u32 total = (u32)ve_enc_total_bytes(g);
	u32 at = MEMORY_BASE + below((MEMORY_END - MEMORY_BASE - total) / VE_ENC_PAGE) * VE_ENC_PAGE;
	unsigned int set;

	a->input = at;
	at += g->input_bytes;
	a->stream = at;
	at += g->stream_bytes;
	for (set = 0; set < VE_ENC_SETS; set++) {
		a->rec[set] = at;
		at += g->rec_bytes;
		a->subpixel[set] = at;
		at += g->subpixel_bytes;
	}
	a->mb_info = at;
}

/*
 * A second account of which pass a picture refers to, in the interface's
 * own words: every pass has a number, and every set of buffers the number
 * of the pass that wrote it.
 */
struct account {
	/* The pass the last picture's passes refer to; 0 for none. */
	unsigned int reference;
	/* The last pass, and the one before it if over the same picture. */
	unsigned int last;
	unsigned int earlier;
	unsigned int holds[VE_ENC_SETS];
	unsigned int passes;
};

/* The set that holds a pass, or VE_ENC_SETS. */
static unsigned int set_of(const struct account *account, unsigned int pass)
{
	unsigned int set;

	for (set = 0; pass && set < VE_ENC_SETS; set++)
		if (account->holds[set] == pass)
			return set;
	return VE_ENC_SETS;
}

/*
 * What the interface says of a request: 0 if it is refused, else 1 and the
 * pass the picture refers to (0 for a key frame that refers to none).
 */
static int account_takes(const struct account *account, u32 flags, int have_picture,
			 unsigned int *reference)
{
	*reference = account->reference;
	if (flags & VE_ENC_AGAIN) {
		if ((flags & (VE_ENC_EARLIER | VE_ENC_STAY)) || !have_picture || !account->last)
			return 0;
	} else if ((flags & VE_ENC_EARLIER) && (flags & VE_ENC_STAY)) {
		return 0;
	} else if (flags & VE_ENC_EARLIER) {
		if (!account->earlier)
			return 0;
		*reference = account->earlier;
	} else if (!(flags & VE_ENC_STAY) && account->last) {
		*reference = account->last;
	}
	return (flags & VE_ENC_KEY) || *reference;
}

static int random_requests(unsigned int count)
{
	static struct ve_enc_op ops[VE_ENC_MAX_OPS];
	unsigned int done = 0, planned = 0, refused_sizes = 0, refused_frames = 0;
	unsigned int streams = 0, failures = 0;

	while (done < count) {
		struct ve_enc_geometry geometry;
		struct ve_enc_addresses addresses;
		struct ve_enc_sets sets;
		struct account account;
		u32 width, height, flags;
		unsigned int length, step;
		int valid_size;

		/*
		 * Most sizes are ones the interface has; one in eight is near
		 * its limits, odd or not, and one in sixteen is anything.
		 */
		width = VE_ENC_MIN_WIDTH + 2 * below((VE_ENC_MAX_WIDTH - VE_ENC_MIN_WIDTH) / 2 + 1);
		height = VE_ENC_MIN_HEIGHT + 2 * below((VE_ENC_MAX_HEIGHT - VE_ENC_MIN_HEIGHT) / 2 + 1);
		flags = below(2);
		switch (below(16)) {
		case 0:
			width = below(VE_ENC_MAX_WIDTH + 64);
			break;
		case 1:
			height = below(VE_ENC_MAX_HEIGHT + 64);
			break;
		case 2:
			width = next();
			height = next();
			flags = next();
			break;
		default:
			break;
		}
		valid_size = width >= VE_ENC_MIN_WIDTH && width <= VE_ENC_MAX_WIDTH &&
			     height >= VE_ENC_MIN_HEIGHT && height <= VE_ENC_MAX_HEIGHT &&
			     !(width & 1) && !(height & 1) && !(flags & ~VE_ENC_CONFIG_FLAGS);
		if ((ve_enc_geometry(width, height, flags, &geometry) == 0) != valid_size) {
			fprintf(stderr, "%ux%u flags %#x: the check and the interface disagree\n",
				width, height, flags);
			return 0;
		}
		done++;
		if (!valid_size) {
			refused_sizes++;
			continue;
		}
		if (!sizes_hold(&geometry))
			return 0;
		place(&geometry, &addresses);
		streams++;

		/* A stream: passes one after the other, as the driver keeps them. */
		ve_enc_sets_reset(&sets);
		memset(&account, 0, sizeof(account));
		length = 1 + below(40);
		for (step = 0; step < length && done < count; step++, done++) {
			struct ve_enc_unit units[VE_ENC_MAX_UNITS + 1];
			struct ve_enc_request request;
			struct ve_enc_sets before = sets;
			struct ve_enc_pass pass;
			unsigned int at, operations, reference = 0, holder;
			int have_picture, valid_frame;

			/*
			 * Most passes are ones a client sends: a picture, a
			 * second pass, the other search, one of the two ways
			 * to name another reference. One in ten is anything.
			 */
			request.flags = 0;
			if ((!account.last && below(10)) || below(60) == 0)
				request.flags |= VE_ENC_KEY;
			if (account.last && below(3) == 0)
				request.flags |= VE_ENC_AGAIN;
			else if (account.earlier && below(3) == 0)
				request.flags |= VE_ENC_EARLIER;
			else if (below(8) == 0)
				request.flags |= VE_ENC_STAY;
			if (below(2))
				request.flags |= VE_ENC_WIDE;
			if (below(10) == 0)
				request.flags = below(64);
			request.qp = below(VE_ENC_MAX_QP + 1);
			request.unit_count = 1 + below(VE_ENC_MAX_UNITS);
			request.units = units;
			for (at = 0; at <= VE_ENC_MAX_UNITS; at++) {
				unsigned int byte;

				units[at].header = (u8)next();
				units[at].bits = below(8 * VE_ENC_MAX_UNIT_BYTES + 1);
				for (byte = 0; byte < VE_ENC_MAX_UNIT_BYTES; byte++)
					units[at].payload[byte] = (u8)next();
			}
			have_picture = account.passes ? below(16) != 0 : below(8) == 0;
			switch (below(40)) {
			case 0:
				request.flags = next();
				break;
			case 1:
				request.qp = next();
				break;
			case 2:
				request.unit_count = below(2) ? 0 : VE_ENC_MAX_UNITS + 1 + below(1000);
				break;
			case 3:
				units[below(VE_ENC_MAX_UNITS)].bits =
					(u16)(8 * VE_ENC_MAX_UNIT_BYTES + 1 + below(1000));
				break;
			default:
				break;
			}

			valid_frame = !(request.flags & ~VE_ENC_FRAME_FLAGS) &&
				      request.qp <= VE_ENC_MAX_QP && request.unit_count >= 1 &&
				      request.unit_count <= VE_ENC_MAX_UNITS &&
				      account_takes(&account, request.flags, have_picture, &reference);
			for (at = 0; valid_frame && at < request.unit_count; at++)
				if (units[at].bits > 8 * VE_ENC_MAX_UNIT_BYTES)
					valid_frame = 0;
			if ((ve_enc_check_frame(&request, &sets, have_picture, &pass) == 0) != valid_frame) {
				fprintf(stderr, "a frame's check and the interface disagree: flags %#x, pass %u\n",
					request.flags, account.passes);
				return 0;
			}
			if (memcmp(&before, &sets, sizeof(sets))) {
				fprintf(stderr, "a check changed the sets\n");
				return 0;
			}
			if (!valid_frame) {
				refused_frames++;
				continue;
			}

			operations = ve_enc_plan(&geometry, &addresses, &request, &pass, ops, VE_ENC_MAX_OPS);
			if (!holds(&geometry, &addresses, &request, &pass, ops, operations)) {
				fprintf(stderr, "%ux%u, pass %u, flags %#x, QP %u, %u units\n", width,
					height, account.passes, request.flags, request.qp, request.unit_count);
				return 0;
			}
			/* A list that is too short is said to be, not overrun. */
			if (ve_enc_plan(&geometry, &addresses, &request, &pass, ops, operations - 1) != 0) {
				fprintf(stderr, "a plan that does not fit was not refused\n");
				return 0;
			}

			/*
			 * The sets, by the second account: the engine reads the
			 * pass the picture refers to, and writes over neither
			 * that one nor, in a second pass, the one before it.
			 */
			holder = set_of(&account, reference);
			if (!(request.flags & VE_ENC_KEY) && pass.reference != holder) {
				fprintf(stderr, "pass %u reads set %u; its reference is in %u\n",
					account.passes + 1, pass.reference, holder);
				return 0;
			}
			if (pass.own == holder ||
			    ((request.flags & VE_ENC_AGAIN) && pass.own == set_of(&account, account.last))) {
				fprintf(stderr, "pass %u writes over a frame that is still needed (flags %#x)\n",
					account.passes + 1, request.flags);
				return 0;
			}
			planned++;

			if (below(50) == 0) {
				/* The engine did not finish: nothing is a frame. */
				ve_enc_sets_reset(&sets);
				memset(&account, 0, sizeof(account));
				failures++;
				continue;
			}
			ve_enc_sets_done(&sets, request.flags, &pass);
			account.passes++;
			account.earlier = (request.flags & VE_ENC_AGAIN) ? account.last : 0;
			account.last = account.passes;
			account.reference = reference;
			account.holds[pass.own] = account.passes;
		}
	}
	printf("ve-enc plan: %u requests in %u streams: %u planned and checked, %u sizes and %u frames refused, %u failures\n",
	       count, streams, planned, refused_sizes, refused_frames, failures);
	/* Not a test that refuses everything. */
	return planned >= count / 2;
}

/* The description the lab tool's `plan` command prints, up to its `ops` line. */
static int dump(void)
{
	static struct ve_enc_op ops[VE_ENC_MAX_OPS];
	struct ve_enc_unit units[VE_ENC_MAX_UNITS];
	struct ve_enc_geometry geometry;
	struct ve_enc_addresses addresses;
	struct ve_enc_request request = { .units = units };
	struct ve_enc_sets sets;
	struct ve_enc_pass pass = { 0 }, checked;
	unsigned int width = 0, height = 0, flags = 0, operations, at;
	char line[1024];

	memset(&addresses, 0, sizeof(addresses));
	memset(units, 0, sizeof(units));
	while (fgets(line, sizeof(line), stdin)) {
		char name[32], hex[2 * VE_ENC_MAX_UNIT_BYTES + 1];
		unsigned int a, b, c, d;

		if (sscanf(line, "geometry %u %u %u", &width, &height, &flags) == 3)
			continue;
		/* The flags, the QP, the set written and the set read. */
		if (sscanf(line, "frame %u %u %u %u", &a, &b, &c, &d) == 4) {
			request.flags = a;
			request.qp = b;
			pass.own = c;
			pass.reference = d;
			continue;
		}
		if (sscanf(line, "address %31s %x", name, &a) == 2) {
			if (!strcmp(name, "input"))
				addresses.input = a;
			else if (!strcmp(name, "stream"))
				addresses.stream = a;
			else if (sscanf(name, "rec%u", &b) == 1 && b < VE_ENC_SETS)
				addresses.rec[b] = a;
			else if (sscanf(name, "subpixel%u", &b) == 1 && b < VE_ENC_SETS)
				addresses.subpixel[b] = a;
			else if (!strcmp(name, "mb_info"))
				addresses.mb_info = a;
			continue;
		}
		hex[0] = 0;
		if (sscanf(line, "unit %x %u %256s", &a, &b, hex) >= 2 &&
		    request.unit_count < VE_ENC_MAX_UNITS) {
			struct ve_enc_unit *unit = &units[request.unit_count++];

			unit->header = (u8)a;
			unit->bits = (u16)b;
			for (c = 0; c < strlen(hex) / 2 && c < VE_ENC_MAX_UNIT_BYTES; c++) {
				unsigned int byte;

				sscanf(hex + 2 * c, "%2x", &byte);
				unit->payload[c] = (u8)byte;
			}
			continue;
		}
		if (!strncmp(line, "ops", 3))
			break;
	}
	/* The request's own fields are checked as for a stream that has a frame. */
	sets.reference = 0;
	sets.last = 0;
	sets.earlier = VE_ENC_NONE;
	if (ve_enc_geometry(width, height, flags, &geometry) != 0 ||
	    ve_enc_check_frame(&request, &sets, 1, &checked) != 0 ||
	    pass.own >= VE_ENC_SETS || pass.reference >= VE_ENC_SETS) {
		fprintf(stderr, "the description is refused\n");
		return 0;
	}
	printf("sizes %u %u %u\n", geometry.picture_bytes, geometry.stream_bytes,
	       geometry.rec_luma_bytes);
	operations = ve_enc_plan(&geometry, &addresses, &request, &pass, ops, VE_ENC_MAX_OPS);
	for (at = 0; at < operations; at++) {
		switch (ops[at].kind) {
		case VE_ENC_OP_WRITE:
			printf("write 0x%03x 0x%08x\n", ops[at].reg, ops[at].value);
			break;
		case VE_ENC_OP_UPDATE:
			printf("update 0x%03x 0x%08x 0x%08x\n", ops[at].reg, ops[at].clear, ops[at].value);
			break;
		case VE_ENC_OP_PUT:
			printf("put %u 0x%x\n", ops[at].bits, ops[at].value);
			break;
		default:
			printf("start\n");
			break;
		}
	}
	return operations != 0;
}

int main(int argc, char **argv)
{
	if (argc >= 3 && !strcmp(argv[1], "random")) {
		if (argc == 4)
			state ^= strtoull(argv[3], NULL, 0);
		return random_requests((unsigned int)strtoul(argv[2], NULL, 0)) ? 0 : 1;
	}
	if (argc == 2 && !strcmp(argv[1], "dump"))
		return dump() ? 0 : 1;
	fprintf(stderr, "usage: %s random COUNT [SEED] | dump < DESCRIPTION\n", argv[0]);
	return 2;
}
