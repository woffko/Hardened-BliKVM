/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * The video engine as a device: its registers, interrupt, clocks, reset and
 * SRAM section. Shared by the modules of this directory.
 */
#ifndef HARDENED_BLIKVM_VE_HW_H
#define HARDENED_BLIKVM_VE_HW_H

#include <linux/clk.h>
#include <linux/device.h>
#include <linux/interrupt.h>
#include <linux/io.h>
#include <linux/platform_device.h>
#include <linux/reset.h>
#include <linux/types.h>

/* The mode register and the encoder's interrupt registers. */
#define VE_MODE			0x000
#define VE_MODE_DECODER_MASK	0xf
#define VE_MODE_ENGINE_AVC_ENC	0xb
#define VE_MODE_INPUT_STAGE	BIT(6)
#define VE_MODE_ENCODER		BIT(7)
#define VE_VERSION		0x0f0
#define VE_AVC_INT_EN		0xb14
#define VE_AVC_INT_EN_MASK	0xf
#define VE_AVC_STATUS		0xb1c
#define VE_AVC_STATUS_MASK	0xf

struct ve_hw {
	struct device *dev;
	void __iomem *base;
	phys_addr_t phys;
	resource_size_t size;
	struct clk *ahb_clk;
	struct clk *mod_clk;
	struct clk *ram_clk;
	struct reset_control *rstc;
	int irq;
	bool powered;
};

int ve_hw_probe(struct platform_device *pdev, struct ve_hw *hw,
		irq_handler_t handler, void *data, unsigned long mod_rate);
void ve_hw_remove(struct ve_hw *hw);
int ve_hw_power_on(struct ve_hw *hw);
void ve_hw_power_off(struct ve_hw *hw);
int ve_hw_reset(struct ve_hw *hw);

static inline u32 ve_read(struct ve_hw *hw, u32 reg)
{
	return readl(hw->base + reg);
}

static inline void ve_write(struct ve_hw *hw, u32 reg, u32 value)
{
	writel(value, hw->base + reg);
}

#endif
