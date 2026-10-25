/* SPDX-License-Identifier: GPL-3.0-only */
#include <string.h>

#include "log.h"
#include "model.h"

#define MODE 0x000
#define RESET 0x004
#define DECODER_IP 0x0e0
#define ENCODER_IP 0x0e4
#define VERSION 0x0f0
#define INPUT_BLOCK 0xa00
#define ENCODER_BLOCK 0xb00
#define BLOCKS_END 0xc00
#define PARA0 0xb04
#define INT_ENABLE 0xb14
#define TRIGGER 0xb18
#define STATUS 0xb1c
#define PUT_BITS 0xb20
#define STREAM_START 0xb80
#define STREAM_END 0xb84
#define STREAM_OFFSET 0xb88
#define STREAM_MAX 0xb8c
#define STREAM_LENGTH 0xb90

#define MODE_STUCK 0xc0000000u
#define MODE_ENCODER 0x80u
#define MODE_INPUT 0x40u
#define MODE_MEMORY_WIDTH (3u << 16)
#define MODE_RECONSTRUCTION (1u << 20)
#define RESET_IDLE 0x300u
#define RESET_ENCODER (1u << 24)
#define RESET_ALL 1u
#define STATUS_REST 0x260u
#define STATUS_FINISH 1u
#define STATUS_CLEARABLE 0xfu
#define NO_PREVENTION (1u << 31)
/* A client that reads the status this often instead of waiting gets its end. */
#define POLLS_BEFORE_FINISH 3

const struct model_config MODEL_H616 = {
	.decoder_ip = 0x00033010,
	.encoder_ip = 0x00012011,
	.version = 0,
	.frame_bits = 8 * 1500,
};

/* The bits that can be set, where they are not all (V1.5). */
static const struct {
	uint16_t offset;
	uint32_t mask;
} MASKS[] = {
	{ 0xa00, 0x07ff07ff }, { 0xa04, 0x07ff07ff }, { 0xa78, 0xfffffff0 },
	{ 0xa7c, 0xfffffff8 }, { 0xb10, 0x00ffffff }, { 0xb24, 0x0003f000 },
	{ 0xb28, 0x80ff00ff }, { 0xb2c, 0xf03fffff }, { 0xb3c, 0x00000000 },
	{ 0xb44, 0xffff7fff }, { 0xb48, 0x03ff03ff }, { 0xb4c, 0x03ff03ff },
	{ 0xb60, 0xfffffffc }, { 0xb80, 0xfffffffc }, { 0xb88, 0x00000000 },
	{ 0xb8c, 0xffff0000 }, { 0xba0, 0xfffffffc }, { 0xba4, 0xfffffffc },
	{ 0xbb0, 0xfffffffc }, { 0xbb4, 0xfffffffc }, { 0xbb8, 0xfffffffc },
	{ 0xbbc, 0xfffffffc }, { 0xbc0, 0xfffffffc }, { 0xbc4, 0xfffffffc },
	{ 0xbc8, 0xfffffffc },
};

#define BLOCK_WORDS ((BLOCKS_END - INPUT_BLOCK) / 4)
#define STREAM_ROOM (1u << 20)

static struct {
	uint32_t *back;
	struct model_config config;
	/* What the two blocks hold; the window shows it while selected. */
	uint32_t held[BLOCK_WORDS];
	int selected;
	int encoding;
	int polls;
	/* The bit writer's output. */
	uint8_t stream[STREAM_ROOM];
	size_t stream_bytes;
	uint32_t pending;
	unsigned int pending_bits;
	unsigned int zeros;
} m;

static uint32_t *held(uint32_t offset)
{
	return &m.held[(offset - INPUT_BLOCK) / 4];
}

static uint32_t mask_of(uint32_t offset)
{
	size_t index;

	for (index = 0; index < sizeof(MASKS) / sizeof(MASKS[0]); index++)
		if (MASKS[index].offset == offset)
			return MASKS[index].mask;
	return 0xffffffff;
}

static void show_blocks(void)
{
	if (m.selected)
		memcpy(&m.back[INPUT_BLOCK / 4], m.held, sizeof(m.held));
	else
		memset(&m.back[INPUT_BLOCK / 4], 0, sizeof(m.held));
}

static void reset_blocks(void)
{
	memset(m.held, 0, sizeof(m.held));
	*held(0xb08) = 0x0000001e;
	*held(STATUS) = STATUS_REST;
	*held(STREAM_END) = 0xffffffff;
	*held(STREAM_MAX) = 0xffff0000;
	m.encoding = 0;
	m.polls = 0;
	m.stream_bytes = 0;
	m.pending = 0;
	m.pending_bits = 0;
	m.zeros = 0;
	show_blocks();
}

void model_reset(void)
{
	memset(m.back, 0, MODEL_WINDOW);
	m.back[MODE / 4] = MODE_STUCK | 7;
	m.back[RESET / 4] = RESET_IDLE;
	m.back[0x01c / 4] = 0x00010000;
	m.back[0x030 / 4] = 0x00000200;
	m.back[0x040 / 4] = 0x0000000f;
	m.back[0x080 / 4] = 0x00001c55;
	m.back[0x084 / 4] = 0x000fffff;
	m.back[0x088 / 4] = 0x00008000;
	m.back[DECODER_IP / 4] = m.config.decoder_ip;
	m.back[ENCODER_IP / 4] = m.config.encoder_ip;
	m.back[VERSION / 4] = m.config.version;
	m.selected = 0;
	reset_blocks();
}

void model_start(uint32_t *back, const struct model_config *config)
{
	memset(&m, 0, sizeof(m));
	m.back = back;
	m.config = *config;
	model_reset();
}

void model_select(int encoder)
{
	uint32_t mode = m.back[MODE / 4] & ~(MODE_ENCODER | MODE_INPUT);

	if (encoder) {
		/* As on the board: a pulse of the encoder's reset, then the bits. */
		reset_blocks();
		mode |= MODE_ENCODER | MODE_INPUT;
	}
	m.back[MODE / 4] = mode;
	m.selected = encoder;
	show_blocks();
}

void model_memory_mode(unsigned int width, int reconstruction_mode)
{
	uint32_t mode = m.back[MODE / 4] & ~(MODE_MEMORY_WIDTH | MODE_RECONSTRUCTION);

	mode |= (width & 3) << 16;
	if (reconstruction_mode)
		mode |= MODE_RECONSTRUCTION;
	m.back[MODE / 4] = mode;
}

/* One byte of the bit writer's output, with emulation prevention if on. */
static void put_byte(uint8_t byte)
{
	int prevention = !(*held(PARA0) & NO_PREVENTION);

	if (prevention && m.zeros >= 2 && byte <= 3) {
		if (m.stream_bytes < STREAM_ROOM)
			m.stream[m.stream_bytes++] = 3;
		*held(STREAM_LENGTH) += 8;
		m.zeros = 0;
	}
	if (m.stream_bytes < STREAM_ROOM)
		m.stream[m.stream_bytes++] = byte;
	m.zeros = byte == 0 ? m.zeros + 1 : 0;
}

static void put_bits(uint32_t value, unsigned int bits)
{
	while (bits) {
		bits--;
		m.pending = (m.pending << 1) | ((value >> bits) & 1);
		if (++m.pending_bits == 8) {
			put_byte((uint8_t)m.pending);
			m.pending = 0;
			m.pending_bits = 0;
		}
		*held(STREAM_LENGTH) += 1;
	}
}

static void put_exp_golomb(uint32_t code)
{
	unsigned int length = 0;

	while ((code + 1) >> (length + 1))
		length++;
	put_bits(0, length);
	put_bits(code + 1, length + 1);
}

static void trigger(uint32_t raw)
{
	uint32_t data = *held(PUT_BITS);
	unsigned int bits = (raw >> 8) & 0x3f;

	switch (raw & 0xf) {
	case 0:
		break;
	case 1:
		put_bits(data, bits > 32 ? 32 : bits);
		break;
	case 2:
		if (m.pending_bits)
			put_bits(0, 8 - m.pending_bits);
		break;
	case 4: {
		int32_t value = (int32_t)data;

		put_exp_golomb(value > 0 ? (uint32_t)value * 2 - 1 : (uint32_t)(-value) * 2);
		break;
	}
	case 5:
		put_exp_golomb(data);
		break;
	case 8:
	case 9:
		m.encoding = 1;
		m.polls = 0;
		log_add(REC_NOTE, "model: an encode was started", raw, 0, 0);
		break;
	default:
		log_add(REC_NOTE, "model: a trigger the maps do not name", raw, 0, 0);
		break;
	}
}

static void finish(void)
{
	m.encoding = 0;
	*held(STATUS) |= STATUS_FINISH;
	*held(STREAM_LENGTH) += m.config.frame_bits;
	log_add(REC_NOTE, "model: the encode finished", *held(STREAM_LENGTH), 0, 0);
	show_blocks();
}

int model_wait(void)
{
	if (!m.encoding)
		return 0;
	finish();
	return 1;
}

void model_read(uint32_t offset, uint32_t value)
{
	(void)value;
	if (offset == STATUS && m.selected && m.encoding && ++m.polls >= POLLS_BEFORE_FINISH)
		finish();
}

static uint32_t write_block(uint32_t offset, uint32_t raw)
{
	uint32_t *word = held(offset);
	uint32_t mask = mask_of(offset);

	if (!m.selected)
		return 0;
	switch (offset) {
	case TRIGGER:
		*word = raw;
		trigger(raw);
		break;
	case STATUS:
		/* Written back to clear; the rest bits stay. */
		*word &= ~(raw & STATUS_CLEARABLE);
		break;
	case STREAM_END:
		*word = raw | 3;
		break;
	case STREAM_START:
		*word = raw & mask;
		m.stream_bytes = 0;
		m.pending = 0;
		m.pending_bits = 0;
		m.zeros = 0;
		break;
	case STREAM_LENGTH:
		*word = raw;
		break;
	default:
		*word = (*word & ~mask) | (raw & mask);
		break;
	}
	show_blocks();
	return *word;
}

uint32_t model_write(uint32_t offset, uint32_t raw, uint32_t old)
{
	if (offset >= INPUT_BLOCK && offset < BLOCKS_END)
		return write_block(offset, raw);
	switch (offset) {
	case MODE: {
		int encoder = (raw & (MODE_ENCODER | MODE_INPUT)) == (MODE_ENCODER | MODE_INPUT);

		if (encoder != m.selected) {
			m.selected = encoder;
			show_blocks();
		}
		return raw | MODE_STUCK;
	}
	case RESET:
		if (raw & RESET_ALL) {
			model_reset();
			return RESET_IDLE;
		}
		if (raw & RESET_ENCODER)
			reset_blocks();
		return raw | RESET_IDLE;
	case DECODER_IP:
	case ENCODER_IP:
	case VERSION:
		return old;
	default:
		return raw;
	}
}

const uint8_t *model_stream(size_t *bytes)
{
	*bytes = m.stream_bytes;
	return m.stream;
}
