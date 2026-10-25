// SPDX-License-Identifier: GPL-2.0-only
/*
 * The video engine as a device. What is taken follows the probe of the
 * mainline decoder driver for the same engine
 * (drivers/staging/media/sunxi/cedrus/cedrus_hw.c, Linux 5.19): the three
 * clocks, the reset, the registers, the interrupt, the SRAM section. Power is
 * the clocks and the reset, in that driver's order; with the reset asserted
 * the engine does nothing and has forgotten everything.
 */
#include <linux/dma-mapping.h>
#include <linux/module.h>
#include <linux/soc/sunxi/sunxi_sram.h>

#include "ve_hw.h"

int ve_hw_probe(struct platform_device *pdev, struct ve_hw *hw,
		irq_handler_t handler, void *data, unsigned long mod_rate)
{
	struct device *dev = &pdev->dev;
	struct resource *res;
	int ret;

	hw->dev = dev;
	hw->powered = false;

	hw->irq = platform_get_irq(pdev, 0);
	if (hw->irq <= 0)
		return hw->irq ? hw->irq : -ENXIO;

	ret = dma_set_mask_and_coherent(dev, DMA_BIT_MASK(32));
	if (ret)
		return dev_err_probe(dev, ret, "no 32-bit DMA\n");

	hw->ahb_clk = devm_clk_get(dev, "ahb");
	if (IS_ERR(hw->ahb_clk))
		return dev_err_probe(dev, PTR_ERR(hw->ahb_clk), "no ahb clock\n");
	hw->mod_clk = devm_clk_get(dev, "mod");
	if (IS_ERR(hw->mod_clk))
		return dev_err_probe(dev, PTR_ERR(hw->mod_clk), "no mod clock\n");
	hw->ram_clk = devm_clk_get(dev, "ram");
	if (IS_ERR(hw->ram_clk))
		return dev_err_probe(dev, PTR_ERR(hw->ram_clk), "no ram clock\n");

	hw->rstc = devm_reset_control_get(dev, NULL);
	if (IS_ERR(hw->rstc))
		return dev_err_probe(dev, PTR_ERR(hw->rstc), "no reset\n");

	res = platform_get_resource(pdev, IORESOURCE_MEM, 0);
	hw->base = devm_ioremap_resource(dev, res);
	if (IS_ERR(hw->base))
		return dev_err_probe(dev, PTR_ERR(hw->base), "no registers\n");
	hw->phys = res->start;
	hw->size = resource_size(res);

	ret = clk_set_rate(hw->mod_clk, mod_rate);
	if (ret)
		return dev_err_probe(dev, ret, "no mod clock rate\n");

	/* The engine is off, so nothing can arrive before power is given. */
	ret = devm_request_irq(dev, hw->irq, handler, 0, dev_name(dev), data);
	if (ret)
		return dev_err_probe(dev, ret, "no interrupt\n");

	ret = sunxi_sram_claim(dev);
	if (ret)
		return dev_err_probe(dev, ret, "no SRAM section\n");

	return 0;
}

void ve_hw_remove(struct ve_hw *hw)
{
	ve_hw_power_off(hw);
	sunxi_sram_release(hw->dev);
}

int ve_hw_power_on(struct ve_hw *hw)
{
	int ret;

	if (hw->powered)
		return 0;

	ret = clk_prepare_enable(hw->ahb_clk);
	if (ret)
		return ret;
	ret = clk_prepare_enable(hw->mod_clk);
	if (ret)
		goto ahb_off;
	ret = clk_prepare_enable(hw->ram_clk);
	if (ret)
		goto mod_off;
	ret = reset_control_reset(hw->rstc);
	if (ret)
		goto ram_off;

	hw->powered = true;
	return 0;

ram_off:
	clk_disable_unprepare(hw->ram_clk);
mod_off:
	clk_disable_unprepare(hw->mod_clk);
ahb_off:
	clk_disable_unprepare(hw->ahb_clk);
	return ret;
}

void ve_hw_power_off(struct ve_hw *hw)
{
	if (!hw->powered)
		return;

	hw->powered = false;
	reset_control_assert(hw->rstc);
	clk_disable_unprepare(hw->ram_clk);
	clk_disable_unprepare(hw->mod_clk);
	clk_disable_unprepare(hw->ahb_clk);
}

/* Everything in the engine back to its state after power-on. */
int ve_hw_reset(struct ve_hw *hw)
{
	if (!hw->powered)
		return -ENODEV;
	return reset_control_reset(hw->rstc);
}
