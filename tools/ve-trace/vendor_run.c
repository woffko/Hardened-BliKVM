/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * The vendor's encoder library, run as a black box with no engine.
 *
 * It is called through its published interface (the interface headers and
 * the vendor's API guide) to encode a few pictures. Where it expects the
 * engine's registers it gets the closed window, behind which the stand-in
 * answers; where it expects the engine's keeper and the allocator it gets
 * vendor_ops.c. What it reads and writes there is the trace. Nothing of
 * the library is read, decoded or kept: it is loaded, called and unloaded.
 *
 * usage: ve-trace-vendor LIBRARY OUTPUT_DIR [NAME=VALUE ...]
 *
 * The library's own messages go to standard output. OUTPUT_DIR gets
 * `trace.bin` (log.h) and `run.txt` (what was asked and what came back).
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "vencoder.h"

#include "harness.h"
#include "log.h"
#include "vendor_ops.h"
#include "window.h"

struct setting {
	const char *name;
	uint64_t value;
	const char *what;
};

static struct setting settings[] = {
	{ "width", 320, "picture width" },
	{ "height", 240, "picture height" },
	{ "frames", 3, "pictures to encode" },
	{ "key_every", 0, "a key frame forced every so many pictures; 0: the library decides" },
	{ "qp", 30, "fixed QP of key frames; 0: rate control" },
	{ "pqp", 30, "fixed QP of the other frames" },
	{ "bitrate", 2000000, "bits per second" },
	{ "fps", 30, "frames per second" },
	{ "gop", 30, "the largest key frame interval" },
	{ "cabac", 0, "1: CABAC" },
	{ "profile", 66, "profile_idc" },
	{ "level", 31, "level_idc" },
	{ "codec", VENC_CODEC_H264, "the library's codec number" },
	{ "format", VENC_PIXEL_YUV420SP, "the library's input format number" },
	{ "give_ops", 1, "1: our tables are put into the configuration" },
	{ "ic", 0x0003301000012011ull, "the engine's version as its keeper tells it" },
	{ "chip", 0, "the chip id the keeper tells" },
	{ "ddr", DDRTYPE_DDR2_32BITS, "the memory type the keeper tells (the H6 keeper's own choice)" },
	{ "offset", 0, "taken off an address before the engine gets it" },
	{ "base", 0x6b000000, "where the made-up addresses begin" },
	{ "decoder_ip", 0x00033010, "what register 0x0e0 reads" },
	{ "encoder_ip", 0x00012011, "what register 0x0e4 reads" },
	{ "version", 0, "what register 0x0f0 reads" },
	{ "frame_bits", 12000, "the bits the stand-in reports for an encode" },
	{ "timeout", 120, "seconds before the run is given up" },
};

static uint64_t setting(const char *name)
{
	size_t index;

	for (index = 0; index < sizeof(settings) / sizeof(settings[0]); index++)
		if (!strcmp(settings[index].name, name))
			return settings[index].value;
	abort();
}

static int take(const char *argument)
{
	const char *equals = strchr(argument, '=');
	size_t index;

	if (!equals)
		return 0;
	for (index = 0; index < sizeof(settings) / sizeof(settings[0]); index++) {
		size_t length = strlen(settings[index].name);

		if ((size_t)(equals - argument) == length && !strncmp(argument, settings[index].name, length)) {
			char *end;

			errno = 0;
			settings[index].value = strtoull(equals + 1, &end, 0);
			return !errno && *end == 0 && end != equals + 1;
		}
	}
	return 0;
}

/* The library may not open files: its request is recorded and refused. */
FILE *fopen(const char *path, const char *mode)
{
	char name[RECORD_NAME] = "fopen refused: ";
	size_t length = strlen(path);
	size_t room = sizeof(name) - strlen(name) - 1;

	(void)mode;
	strncat(name, length > room ? path + length - room : path, room);
	log_add(REC_OP, name, 0, 0, 0);
	errno = EACCES;
	return NULL;
}

static int report = -1;

static void say(const char *format, ...) __attribute__((format(printf, 1, 2)));
static void say(const char *format, ...)
{
	va_list arguments;

	va_start(arguments, format);
	vdprintf(report, format, arguments);
	va_end(arguments);
}

static void say_bytes(const char *name, const unsigned char *bytes, size_t count)
{
	size_t index;

	say("%s (%zu bytes):", name, count);
	for (index = 0; index < count && index < 256; index++)
		say(" %02x", bytes[index]);
	say("%s\n", count > 256 ? " ..." : "");
}

static void on_alarm(int number)
{
	(void)number;
	harness_fatal("the run took too long");
}

/* A function of the library by name; the run ends if it is missing. */
static void *symbol(void *library, const char *name)
{
	void *found = dlsym(library, name);

	if (!found) {
		say("missing: %s\n", name);
		harness_fatal("the library lacks a function of its interface");
	}
	return found;
}

#define CALL(name, expression)                                        \
	({                                                            \
		long long result_;                                    \
		log_add(REC_CALL, name, 0, 0, 0);                     \
		result_ = (long long)(expression);                    \
		log_add(REC_RETURN, name, (uint64_t)result_, 0, 0);   \
		say("%s -> %lld\n", name, result_);                   \
		result_;                                              \
	})

int main(int argc, char **argv)
{
	VideoEncoder *(*create)(VENC_CODEC_TYPE);
	void (*destroy)(VideoEncoder *);
	int (*init)(VideoEncoder *, VencBaseConfig *);
	int (*uninit)(VideoEncoder *);
	int (*allocate)(VideoEncoder *, VencAllocateBufferParam *);
	int (*get_input)(VideoEncoder *, VencInputBuffer *);
	int (*flush_input)(VideoEncoder *, VencInputBuffer *);
	int (*return_input)(VideoEncoder *, VencInputBuffer *);
	int (*add_input)(VideoEncoder *, VencInputBuffer *);
	int (*encode)(VideoEncoder *);
	int (*used_input)(VideoEncoder *, VencInputBuffer *);
	int (*get_stream)(VideoEncoder *, VencOutputBuffer *);
	int (*free_stream)(VideoEncoder *, VencOutputBuffer *);
	int (*get_parameter)(VideoEncoder *, VENC_INDEXTYPE, void *);
	int (*set_parameter)(VideoEncoder *, VENC_INDEXTYPE, void *);
	struct model_config model = MODEL_H616;
	struct vendor_answers answers;
	VencBaseConfig config;
	VencH264Param h264;
	VencAllocateBufferParam buffers;
	VencHeaderData header;
	VideoEncoder *encoder;
	char path[4096];
	unsigned int width, height, stride, rows, frame;
	void *library;
	size_t index;
	int argument;

	if (argc < 3) {
		fprintf(stderr, "usage: %s LIBRARY OUTPUT_DIR [NAME=VALUE ...]\n", argv[0]);
		for (index = 0; index < sizeof(settings) / sizeof(settings[0]); index++)
			fprintf(stderr, "  %-11s %-18llu %s\n", settings[index].name,
				(unsigned long long)settings[index].value, settings[index].what);
		return 2;
	}
	for (argument = 3; argument < argc; argument++) {
		if (!take(argv[argument])) {
			fprintf(stderr, "not a setting: %s\n", argv[argument]);
			return 2;
		}
	}

	snprintf(path, sizeof(path), "%s/run.txt", argv[2]);
	report = open(path, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
	if (report < 0) {
		perror(path);
		return 1;
	}
	for (index = 0; index < sizeof(settings) / sizeof(settings[0]); index++)
		say("setting %s = %#llx\n", settings[index].name, (unsigned long long)settings[index].value);

	model.decoder_ip = (uint32_t)setting("decoder_ip");
	model.encoder_ip = (uint32_t)setting("encoder_ip");
	model.version = (uint32_t)setting("version");
	model.frame_bits = (uint32_t)setting("frame_bits");
	snprintf(path, sizeof(path), "%s/trace.bin", argv[2]);
	if (!harness_start(&model, path) || !arena_start(96u << 20, setting("base"))) {
		fprintf(stderr, "the window or the arena cannot be set up\n");
		return 1;
	}
	answers.ic_version = setting("ic");
	answers.chip_id = (int)setting("chip");
	answers.dram_type = (int)setting("ddr");
	answers.address_offset = (uint32_t)setting("offset");
	vendor_ops_start(&answers);
	signal(SIGALRM, on_alarm);
	alarm((unsigned int)setting("timeout"));

	library = dlopen(argv[1], RTLD_NOW | RTLD_GLOBAL);
	if (!library) {
		say("dlopen: %s\n", dlerror());
		harness_fatal("the library cannot be loaded");
	}
	create = symbol(library, "VideoEncCreate");
	destroy = symbol(library, "VideoEncDestroy");
	init = symbol(library, "VideoEncInit");
	uninit = symbol(library, "VideoEncUnInit");
	allocate = symbol(library, "AllocInputBuffer");
	get_input = symbol(library, "GetOneAllocInputBuffer");
	flush_input = symbol(library, "FlushCacheAllocInputBuffer");
	return_input = symbol(library, "ReturnOneAllocInputBuffer");
	add_input = symbol(library, "AddOneInputBuffer");
	encode = symbol(library, "VideoEncodeOneFrame");
	used_input = symbol(library, "AlreadyUsedInputBuffer");
	get_stream = symbol(library, "GetOneBitstreamFrame");
	free_stream = symbol(library, "FreeOneBitStreamFrame");
	get_parameter = symbol(library, "VideoEncGetParameter");
	set_parameter = symbol(library, "VideoEncSetParameter");

	width = (unsigned int)setting("width");
	height = (unsigned int)setting("height");
	stride = (width + 15) & ~15u;
	rows = (height + 15) & ~15u;

	log_add(REC_CALL, "VideoEncCreate", 0, 0, 0);
	encoder = create((VENC_CODEC_TYPE)setting("codec"));
	log_add(REC_RETURN, "VideoEncCreate", encoder != NULL, 0, 0);
	say("VideoEncCreate -> %s\n", encoder ? "an encoder" : "nothing");
	if (!encoder)
		harness_fatal("the library made no encoder");

	memset(&h264, 0, sizeof(h264));
	h264.sProfileLevel.nProfile = (VENC_H264PROFILETYPE)setting("profile");
	h264.sProfileLevel.nLevel = (VENC_H264LEVELTYPE)setting("level");
	h264.bEntropyCodingCABAC = (int)setting("cabac");
	h264.sQPRange.nMinqp = 10;
	h264.sQPRange.nMaxqp = 40;
	h264.nFramerate = (int)setting("fps");
	h264.nBitrate = (int)setting("bitrate");
	h264.nMaxKeyInterval = (int)setting("gop");
	h264.nCodingMode = VENC_FRAME_CODING;
	CALL("VideoEncSetParameter(H264Param)", set_parameter(encoder, VENC_IndexParamH264Param, &h264));
	if (setting("qp")) {
		VencH264FixQP fixed = { 1, (int)setting("qp"), (int)setting("pqp") };

		CALL("VideoEncSetParameter(H264FixQP)", set_parameter(encoder, VENC_IndexParamH264FixQP, &fixed));
	}

	memset(&config, 0, sizeof(config));
	config.nInputWidth = width;
	config.nInputHeight = height;
	config.nDstWidth = width;
	config.nDstHeight = height;
	config.nStride = stride;
	config.eInputFormat = (VENC_PIXEL_FMT)setting("format");
	config.memops = vendor_memory_ops();
	if (setting("give_ops")) {
		config.veOpsS = vendor_engine_ops();
		config.pVeOpsSelf = vendor_engine_self();
	}
	if (CALL("VideoEncInit", init(encoder, &config)) != 0)
		harness_fatal("the library did not initialise");

	memset(&header, 0, sizeof(header));
	if (CALL("VideoEncGetParameter(H264SPSPPS)", get_parameter(encoder, VENC_IndexParamH264SPSPPS, &header)) == 0 &&
	    header.pBuffer)
		say_bytes("parameter sets", header.pBuffer, header.nLength);

	memset(&buffers, 0, sizeof(buffers));
	buffers.nBufferNum = 1;
	buffers.nSizeY = stride * rows;
	buffers.nSizeC = stride * rows / 2;
	if (CALL("AllocInputBuffer", allocate(encoder, &buffers)) != 0)
		harness_fatal("the library gave no input buffer");

	for (frame = 0; frame < setting("frames"); frame++) {
		VencInputBuffer input;
		VencOutputBuffer output;
		long long result;

		say("frame %u\n", frame);
		log_add(REC_NOTE, "frame", frame, 0, 0);
		memset(&input, 0, sizeof(input));
		if (CALL("GetOneAllocInputBuffer", get_input(encoder, &input)) != 0)
			harness_fatal("the library gave no input buffer");
		say("input: luma %#llx (buffer %d), chroma %#llx (buffer %d)\n",
		    (unsigned long long)(uintptr_t)input.pAddrPhyY, arena_buffer(input.pAddrVirY),
		    (unsigned long long)(uintptr_t)input.pAddrPhyC, arena_buffer(input.pAddrVirC));
		if (input.pAddrVirY && input.pAddrVirC) {
			memset(input.pAddrVirY, 0x40 + (int)frame, buffers.nSizeY);
			memset(input.pAddrVirC, 0x80, buffers.nSizeC);
		}
		input.nPts = (long long)frame * 1000000 / (long long)setting("fps");
		input.bEnableCorp = 0;
		CALL("FlushCacheAllocInputBuffer", flush_input(encoder, &input));
		if (setting("key_every") && frame && frame % setting("key_every") == 0) {
			int force = 1;

			CALL("VideoEncSetParameter(ForceKeyFrame)",
			     set_parameter(encoder, VENC_IndexParamForceKeyFrame, &force));
		}
		CALL("AddOneInputBuffer", add_input(encoder, &input));
		result = CALL("VideoEncodeOneFrame", encode(encoder));
		CALL("AlreadyUsedInputBuffer", used_input(encoder, &input));
		CALL("ReturnOneAllocInputBuffer", return_input(encoder, &input));
		if (result != VENC_RESULT_OK)
			continue;
		memset(&output, 0, sizeof(output));
		if (CALL("GetOneBitstreamFrame", get_stream(encoder, &output)) != 0)
			continue;
		say("output: flag %#x, %u bytes at buffer %d address %#llx, %u bytes at buffer %d address %#llx\n",
		    output.nFlag, output.nSize0, arena_buffer(output.pData0),
		    (unsigned long long)arena_address(output.pData0), output.nSize1,
		    arena_buffer(output.pData1), (unsigned long long)arena_address(output.pData1));
		if (output.pData0 && output.nSize0 && arena_buffer(output.pData0) >= 0)
			say_bytes("output begins", output.pData0, output.nSize0 < 64 ? output.nSize0 : 64);
		CALL("FreeOneBitStreamFrame", free_stream(encoder, &output));
	}

	{
		size_t bytes;
		const uint8_t *stream = model_stream(&bytes);

		say_bytes("the bit writer's last output", stream, bytes);
	}
	CALL("VideoEncUnInit", uninit(encoder));
	log_add(REC_CALL, "VideoEncDestroy", 0, 0, 0);
	destroy(encoder);
	log_add(REC_RETURN, "VideoEncDestroy", 0, 0, 0);
	alarm(0);
	say("accesses: %llu\n", (unsigned long long)window_accesses());
	if (!harness_finish()) {
		fprintf(stderr, "the trace could not be written\n");
		return 1;
	}
	return 0;
}
