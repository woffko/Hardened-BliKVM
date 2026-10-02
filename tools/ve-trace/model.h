/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * A stand-in for the H616's video engine behind the register window: what
 * this project measured on the board (docs/design/h616-ve-h264-encoder.md,
 * V1.3 and V1.5) and what the open register maps say. It encodes nothing.
 * It answers: registers hold the bits the engine's hold, the encoder's
 * blocks read zero until the encoder is selected, the bit writer counts
 * its bits, and an encode that was started ends with the finish status
 * when its client waits for the interrupt.
 */
#ifndef VE_TRACE_MODEL_H
#define VE_TRACE_MODEL_H

#include <stddef.h>
#include <stdint.h>

#define MODEL_WINDOW 0x2000

struct model_config {
	/* What `0x0e0`, `0x0e4` and `0x0f0` read. */
	uint32_t decoder_ip;
	uint32_t encoder_ip;
	uint32_t version;
	/* The bits an encode adds to the length register. */
	uint32_t frame_bits;
};

extern const struct model_config MODEL_H616;

/* `back` is the window's open view, MODEL_WINDOW bytes. */
void model_start(uint32_t *back, const struct model_config *config);
/* The window's hooks. */
uint32_t model_write(uint32_t offset, uint32_t raw, uint32_t old);
void model_read(uint32_t offset, uint32_t value);
/* The kernel's part: the engine's reset line. */
void model_reset(void);
/* The kernel's or the engine library's part: the encoder switched on or off. */
void model_select(int encoder);
/*
 * The keeper's part: the memory interface field of the mode register (bits
 * 17:16) and the reconstruction write mode (bit 20).
 */
void model_memory_mode(unsigned int width, int reconstruction_mode);
/*
 * A client waits for the encoder's interrupt. 1 if an encode was under way
 * and has now finished, 0 if nothing was.
 */
int model_wait(void);
/* What the bit writer has put out since the stream registers were set. */
const uint8_t *model_stream(size_t *bytes);

#endif
