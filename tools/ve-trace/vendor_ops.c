/* SPDX-License-Identifier: GPL-3.0-only */
/*
 * Our side of the vendor encoder library's interface.
 *
 * The library takes two tables of functions from its caller: what it asks
 * of the engine's keeper (lock, reset, wait for the interrupt, where the
 * registers are, which engine this is) and what it asks of the allocator
 * (buffers and the addresses the engine is to be told). Its own companion
 * libraries would fill them with a kernel driver behind; here they are
 * filled with the stand-in and the arena, and every call is recorded.
 *
 * This file is compiled against the library's interface headers, which
 * stay in the cache outside the repository. Nothing of them is copied.
 */
#include <string.h>

#include "veInterface.h"
#include "sc_interface.h"
#include "veAdapter.h"
#include "memoryAdapter.h"

#include "harness.h"
#include "log.h"
#include "vendor_ops.h"
#include "window.h"

static struct vendor_answers answers;
static int self;

/* Where the engine's blocks begin in its window (the open register maps). */
static const struct {
	int group;
	uint32_t offset;
} GROUPS[] = {
	{ REG_GROUP_VETOP, 0x000 },       { REG_GROUP_MPEG_DECODER, 0x100 },
	{ REG_GROUP_H264_DECODER, 0x200 }, { REG_GROUP_VC1_DECODER, 0x300 },
	{ REG_GROUP_RV_DECODER, 0x400 },   { REG_GROUP_H265_DECODER, 0x500 },
	{ REG_GROUP_JPEG_DECODER, 0xe00 },
};

static void op(const char *name, uint64_t a, uint64_t b, uint64_t c)
{
	log_add(REC_OP, name, a, b, c);
}

/* The engine's keeper. */

static void *ve_init(VeConfig *config)
{
	if (config) {
		op("ve.init", (uint64_t)config->nEncoderFlag | (uint64_t)config->nDecoderFlag << 1,
		   (uint64_t)config->nFormat, (uint64_t)config->nWidth);
		op("ve.init: afbc, reset mode, freq", (uint64_t)config->nEnableAfbcFlag,
		   (uint64_t)config->nResetVeMode, config->nVeFreq);
	} else {
		op("ve.init: no config", 0, 0, 0);
	}
	return &self;
}

static void ve_release(void *p)
{
	(void)p;
	op("ve.release", 0, 0, 0);
}

static int ve_lock(void *p)
{
	(void)p;
	op("ve.lock", 0, 0, 0);
	return 0;
}

static int ve_unlock(void *p)
{
	(void)p;
	op("ve.unlock", 0, 0, 0);
	return 0;
}

static void ve_reset(void *p)
{
	(void)p;
	engine_reset("ve.reset");
}

static int ve_wait_interrupt(void *p)
{
	(void)p;
	/* The keeper's answer: 0 when the interrupt came. */
	return engine_wait("ve.waitInterrupt") ? 0 : -1;
}

static int ve_get_chip_id(void *p)
{
	(void)p;
	op("ve.getChipId", (uint64_t)answers.chip_id, 0, 0);
	return answers.chip_id;
}

static uint64_t ve_get_ic_version(void *p)
{
	(void)p;
	op("ve.getIcVeVersion", answers.ic_version, 0, 0);
	return answers.ic_version;
}

static void *ve_get_group_reg_addr(void *p, int group)
{
	size_t index;

	(void)p;
	for (index = 0; index < sizeof(GROUPS) / sizeof(GROUPS[0]); index++) {
		if (GROUPS[index].group != group)
			continue;
		op("ve.getGroupRegAddr", (uint64_t)group, GROUPS[index].offset, 0);
		return (uint8_t *)window_guest() + GROUPS[index].offset;
	}
	op("ve.getGroupRegAddr: unknown group", (uint64_t)group, 0, 0);
	return window_guest();
}

static int ve_get_dram_type(void *p)
{
	(void)p;
	op("ve.getDramType", (uint64_t)answers.dram_type, 0, 0);
	return answers.dram_type;
}

static unsigned int ve_get_phy_offset(void *p)
{
	(void)p;
	op("ve.getPhyOffset", answers.address_offset, 0, 0);
	return answers.address_offset;
}

/*
 * The memory type becomes the mode register's memory interface field the
 * way the vendor's published keeper for the H6 sets it (H6-CedarC,
 * ve/veAw/veAw.c, LGPL-2.1): 16-bit DDR1 is 0, 32-bit DDR1 and 16-bit DDR2
 * are 1, 32-bit DDR2 and 16-bit DDR3 are 2, wider DDR3 is 3 with the
 * reconstruction write mode on.
 */
static void ve_set_dram_type(void *p)
{
	unsigned int width;

	(void)p;
	switch (answers.dram_type) {
	case DDRTYPE_DDR1_16BITS:
		width = 0;
		break;
	case DDRTYPE_DDR1_32BITS:
	case DDRTYPE_DDR2_16BITS:
		width = 1;
		break;
	case DDRTYPE_DDR2_32BITS:
	case DDRTYPE_DDR3_16BITS:
		width = 2;
		break;
	default:
		width = 3;
		break;
	}
	engine_memory_mode("ve.setDramType", width, width == 3);
}

static void ve_set_ddr_mode(void *p, int mode)
{
	(void)p;
	op("ve.setDdrMode", (uint64_t)mode, 0, 0);
	answers.dram_type = mode >= DDRTYPE_MIN && mode <= DDRTYPE_MAX ? mode : DDRTYPE_DDR3_32BITS;
}

static int ve_set_speed(void *p, unsigned int mhz)
{
	(void)p;
	op("ve.setSpeed", mhz, 0, 0);
	return 0;
}

static void ve_set_enable_afbc_flag(void *p, int flag)
{
	(void)p;
	op("ve.setEnableAfbcFlag", (uint64_t)flag, 0, 0);
}

static void ve_set_adjust_dram_speed_flag(void *p, int flag)
{
	(void)p;
	op("ve.setAdjustDramSpeedFlag", (uint64_t)flag, 0, 0);
}

static void ve_enable(void *p)
{
	(void)p;
	engine_select("ve.enableVe", 1);
}

static void ve_disable(void *p)
{
	(void)p;
	engine_select("ve.disableVe", 0);
}

static void ve_init_encoder_performance(void *p, int mode)
{
	(void)p;
	op("ve.initEncoderPerformance", (uint64_t)mode, 0, 0);
}

static void ve_uninit_encoder_performance(void *p, int mode)
{
	(void)p;
	op("ve.unInitEncoderPerformance", (uint64_t)mode, 0, 0);
}

static int ve_get_iommu_addr(void *p, struct user_iommu_param *param)
{
	(void)p;
	op("ve.getIommuAddr", param ? (uint64_t)param->fd : 0, 0, 0);
	return -1;
}

static int ve_free_iommu_addr(void *p, struct user_iommu_param *param)
{
	(void)p;
	op("ve.freeIommuAddr", param ? (uint64_t)param->fd : 0, 0, 0);
	return -1;
}

static int ve_set_proc_info(void *p, char *text, unsigned int length, unsigned char channel)
{
	(void)p;
	(void)text;
	op("ve.setProcInfo", length, channel, 0);
	return 0;
}

static int ve_stop_proc_info(void *p, unsigned char channel)
{
	(void)p;
	op("ve.stopProcInfo", channel, 0, 0);
	return 0;
}

static VeOpsS engine_ops = {
	.init = ve_init,
	.release = ve_release,
	.lock = ve_lock,
	.unlock = ve_unlock,
	.reset = ve_reset,
	.waitInterrupt = ve_wait_interrupt,
	.getChipId = ve_get_chip_id,
	.getIcVeVersion = ve_get_ic_version,
	.getGroupRegAddr = ve_get_group_reg_addr,
	.getDramType = ve_get_dram_type,
	.getPhyOffset = ve_get_phy_offset,
	.setDramType = ve_set_dram_type,
	.setDdrMode = ve_set_ddr_mode,
	.setSpeed = ve_set_speed,
	.setEnableAfbcFlag = ve_set_enable_afbc_flag,
	.setAdjustDramSpeedFlag = ve_set_adjust_dram_speed_flag,
	.enableVe = ve_enable,
	.disableVe = ve_disable,
	.initEncoderPerformance = ve_init_encoder_performance,
	.unInitEncoderPerformance = ve_uninit_encoder_performance,
	.getIommuAddr = ve_get_iommu_addr,
	.freeIommuAddr = ve_free_iommu_addr,
	.setProcInfo = ve_set_proc_info,
	.stopProcInfo = ve_stop_proc_info,
};

/* The allocator. */

static int mem_open(void)
{
	op("mem.open", 0, 0, 0);
	return 0;
}

static void mem_close(void)
{
	op("mem.close", 0, 0, 0);
}

static int mem_total_size(void)
{
	op("mem.total_size", 0, 0, 0);
	return 64 << 20;
}

static void *mem_palloc(int size, void *ops, void *ops_self)
{
	(void)ops;
	(void)ops_self;
	return size > 0 ? arena_allocate((size_t)size) : NULL;
}

static void mem_pfree(void *memory, void *ops, void *ops_self)
{
	(void)ops;
	(void)ops_self;
	arena_free(memory);
}

/* A place in the arena as the trace tells it: buffer and offset within. */
static void place(const char *name, const void *memory, uint64_t extra)
{
	int number = arena_buffer(memory);
	uint64_t address = arena_address(memory);

	op(name, (uint64_t)(int64_t)number, address, extra);
}

static void mem_flush_cache(void *memory, int size)
{
	place("mem.flush_cache", memory, (uint64_t)size);
}

static void *mem_ve_get_phyaddr(void *virtual_address)
{
	uint64_t address = arena_address(virtual_address);
	uint64_t told = address ? address - answers.address_offset : 0;

	place("mem.ve_get_phyaddr", virtual_address, told);
	return (void *)(uintptr_t)told;
}

static void *mem_ve_get_viraddr(void *address)
{
	void *memory = arena_pointer((uint64_t)(uintptr_t)address + answers.address_offset);

	place("mem.ve_get_viraddr", memory, (uint64_t)(uintptr_t)address);
	return memory;
}

static void *mem_cpu_get_phyaddr(void *virtual_address)
{
	uint64_t address = arena_address(virtual_address);

	place("mem.cpu_get_phyaddr", virtual_address, address);
	return (void *)(uintptr_t)address;
}

static void *mem_cpu_get_viraddr(void *address)
{
	void *memory = arena_pointer((uint64_t)(uintptr_t)address);

	place("mem.cpu_get_viraddr", memory, (uint64_t)(uintptr_t)address);
	return memory;
}

static int mem_set(void *memory, int value, size_t bytes)
{
	memset(memory, value, bytes);
	return 0;
}

static int mem_cpy(void *to, void *from, size_t bytes)
{
	memcpy(to, from, bytes);
	return 0;
}

static int mem_setup(void)
{
	op("mem.setup", 0, 0, 0);
	return 0;
}

static int mem_shutdown(void)
{
	op("mem.shutdown", 0, 0, 0);
	return 0;
}

static unsigned int mem_get_ve_addr_offset(void)
{
	op("mem.get_ve_addr_offset", answers.address_offset, 0, 0);
	return answers.address_offset;
}

static struct ScMemOpsS memory_ops = {
	.open = mem_open,
	.close = mem_close,
	.total_size = mem_total_size,
	.palloc = mem_palloc,
	.pfree = mem_pfree,
	.flush_cache = mem_flush_cache,
	.ve_get_phyaddr = mem_ve_get_phyaddr,
	.ve_get_viraddr = mem_ve_get_viraddr,
	.cpu_get_phyaddr = mem_cpu_get_phyaddr,
	.cpu_get_viraddr = mem_cpu_get_viraddr,
	.mem_set = mem_set,
	.mem_cpy = mem_cpy,
	.mem_read = mem_cpy,
	.mem_write = mem_cpy,
	.setup = mem_setup,
	.shutdown = mem_shutdown,
	.palloc_secure = mem_palloc,
	.get_ve_addr_offset = mem_get_ve_addr_offset,
};

/* What the library's companion libraries export, by their names. */

VeOpsS *GetVeOpsS(int type)
{
	op("GetVeOpsS", (uint64_t)type, 0, 0);
	return &engine_ops;
}

struct ScMemOpsS *MemAdapterGetOpsS(void)
{
	op("MemAdapterGetOpsS", 0, 0, 0);
	return &memory_ops;
}

void vendor_ops_start(const struct vendor_answers *given)
{
	answers = *given;
}

void *vendor_engine_ops(void)
{
	return &engine_ops;
}

void *vendor_engine_self(void)
{
	return &self;
}

void *vendor_memory_ops(void)
{
	return &memory_ops;
}
