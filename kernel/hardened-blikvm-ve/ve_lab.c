// SPDX-License-Identifier: GPL-2.0-only
/*
 * The lab module of the hardware H.264 plan
 * (docs/design/h616-ve-h264-encoder.md). It hands the video engine to one
 * root process: the register window mapped, DMA buffers with their bus
 * addresses, a wait for the encoder's interrupt, a reset. That is what the
 * vendor's kernel gives the vendor's library, and it is as dangerous:
 * whoever programs the engine can make it read and write any memory. It is a
 * lab instrument, loaded for a session and unloaded after it; no package,
 * slot or image carries it.
 *
 * It binds to the engine's node while the decoder driver is unbound.
 * Opening the device powers the engine; closing it, or the client's death,
 * resets the engine, takes the power away and frees every buffer.
 *
 * The interrupt is handled as the vendor kernel's driver handles the
 * encoder's (drivers/media/cedar-ve/cedar_ve.c of the vendor's 4.9 kernel):
 * with the encoder selected and a status bit and an enable bit set, the
 * enable bits are cleared and the waiter is woken. The vendor's driver
 * clears three enable bits; one open source sets four, so four are cleared
 * here. Any other interrupt cannot be quieted from here; the line is
 * switched off until the client closes, and the client is told.
 */
#include <linux/dma-mapping.h>
#include <linux/fs.h>
#include <linux/miscdevice.h>
#include <linux/mm.h>
#include <linux/module.h>
#include <linux/mod_devicetable.h>
#include <linux/mutex.h>
#include <linux/of.h>
#include <linux/slab.h>
#include <linux/spinlock.h>
#include <linux/uaccess.h>
#include <linux/wait.h>

#include "ve_hw.h"
#include "ve_lab_abi.h"

/* The rate the decoder driver gives the engine's clock on this SoC. */
static unsigned long mod_rate = 600000000;
module_param(mod_rate, ulong, 0444);
MODULE_PARM_DESC(mod_rate, "rate of the engine's module clock in Hz");

struct ve_lab_buffer {
	void *cpu;
	dma_addr_t dma;
	size_t size;
};

struct ve_lab {
	struct ve_hw hw;
	struct miscdevice misc;
	/* Open and release, the requests, the mappings. */
	struct mutex lock;
	unsigned long opened;
	struct ve_lab_buffer buffers[VE_LAB_MAX_BUFFERS];
	size_t allocated;
	u32 version;
	/* The interrupt's news. */
	spinlock_t irq_lock;
	wait_queue_head_t wait;
	bool came;
	bool unknown;
	bool line_off;
	u32 status;
};

static struct ve_lab *ve_lab_of(struct file *file)
{
	return container_of(file->private_data, struct ve_lab, misc);
}

static irqreturn_t ve_lab_irq(int irq, void *data)
{
	struct ve_lab *lab = data;
	struct ve_hw *hw = &lab->hw;
	unsigned long flags;
	bool ours = false;
	u32 mode;
	u32 status;

	spin_lock_irqsave(&lab->irq_lock, flags);
	if (hw->powered) {
		mode = ve_read(hw, VE_MODE);
		/* Both ways the open sources select the encoder. */
		if ((mode & VE_MODE_ENCODER) ||
		    (mode & VE_MODE_DECODER_MASK) == VE_MODE_ENGINE_AVC_ENC) {
			status = ve_read(hw, VE_AVC_STATUS) & VE_AVC_STATUS_MASK;
			if (status &&
			    (ve_read(hw, VE_AVC_INT_EN) & VE_AVC_INT_EN_MASK)) {
				ve_write(hw, VE_AVC_INT_EN,
					 ve_read(hw, VE_AVC_INT_EN) &
					 ~VE_AVC_INT_EN_MASK);
				lab->status = status;
				lab->came = true;
				ours = true;
			}
		}
	}
	if (!ours) {
		disable_irq_nosync(irq);
		lab->line_off = true;
		lab->unknown = true;
	}
	spin_unlock_irqrestore(&lab->irq_lock, flags);

	wake_up(&lab->wait);
	return IRQ_HANDLED;
}

static void ve_lab_free_buffers(struct ve_lab *lab)
{
	unsigned int index;

	for (index = 0; index < VE_LAB_MAX_BUFFERS; index++) {
		struct ve_lab_buffer *buffer = &lab->buffers[index];

		if (!buffer->cpu)
			continue;
		dma_free_coherent(lab->hw.dev, buffer->size, buffer->cpu,
				  buffer->dma);
		buffer->cpu = NULL;
		buffer->size = 0;
	}
	lab->allocated = 0;
}

static int ve_lab_open(struct inode *inode, struct file *file)
{
	struct ve_lab *lab = ve_lab_of(file);
	unsigned long flags;
	int ret;

	if (test_and_set_bit(0, &lab->opened))
		return -EBUSY;

	mutex_lock(&lab->lock);
	spin_lock_irqsave(&lab->irq_lock, flags);
	lab->came = false;
	lab->unknown = false;
	lab->status = 0;
	spin_unlock_irqrestore(&lab->irq_lock, flags);

	ret = ve_hw_power_on(&lab->hw);
	if (!ret)
		lab->version = ve_read(&lab->hw, VE_VERSION);
	mutex_unlock(&lab->lock);
	if (ret)
		clear_bit(0, &lab->opened);
	return ret;
}

static int ve_lab_release(struct inode *inode, struct file *file)
{
	struct ve_lab *lab = ve_lab_of(file);
	unsigned long flags;
	bool line_off;

	mutex_lock(&lab->lock);
	/*
	 * No handler may read a register while the clocks go: the line is
	 * off across the power-down. The reset stops whatever the engine was
	 * doing with the buffers, and takes every interrupt source with it.
	 */
	disable_irq(lab->hw.irq);
	ve_hw_power_off(&lab->hw);
	ve_lab_free_buffers(lab);

	spin_lock_irqsave(&lab->irq_lock, flags);
	line_off = lab->line_off;
	lab->line_off = false;
	spin_unlock_irqrestore(&lab->irq_lock, flags);
	/* Once for the handler's own switch-off, once for the one above. */
	if (line_off)
		enable_irq(lab->hw.irq);
	enable_irq(lab->hw.irq);
	mutex_unlock(&lab->lock);

	clear_bit(0, &lab->opened);
	return 0;
}

static long ve_lab_alloc(struct ve_lab *lab, void __user *argument)
{
	struct ve_lab_alloc request;
	struct ve_lab_buffer *buffer = NULL;
	unsigned int index;
	size_t size;

	if (copy_from_user(&request, argument, sizeof(request)))
		return -EFAULT;
	if (!request.bytes || request.bytes > VE_LAB_MAX_BUFFER_BYTES)
		return -EINVAL;
	size = PAGE_ALIGN(request.bytes);
	if (lab->allocated + size > VE_LAB_MAX_TOTAL_BYTES)
		return -ENOSPC;

	for (index = 0; index < VE_LAB_MAX_BUFFERS; index++) {
		if (!lab->buffers[index].cpu) {
			buffer = &lab->buffers[index];
			break;
		}
	}
	if (!buffer)
		return -ENOSPC;

	buffer->cpu = dma_alloc_coherent(lab->hw.dev, size, &buffer->dma,
					 GFP_KERNEL);
	if (!buffer->cpu)
		return -ENOMEM;
	buffer->size = size;
	lab->allocated += size;

	request.bytes = size;
	request.index = index;
	request.address = lower_32_bits(buffer->dma);
	request.reserved = 0;
	request.offset = (u64)(index + 1) << VE_LAB_BUFFER_SHIFT;
	if (upper_32_bits(buffer->dma) ||
	    copy_to_user(argument, &request, sizeof(request))) {
		dma_free_coherent(lab->hw.dev, size, buffer->cpu, buffer->dma);
		buffer->cpu = NULL;
		buffer->size = 0;
		lab->allocated -= size;
		return -EFAULT;
	}
	return 0;
}

static long ve_lab_wait(struct ve_lab *lab, void __user *argument)
{
	struct ve_lab_wait request;
	unsigned long flags;
	long left;

	if (copy_from_user(&request, argument, sizeof(request)))
		return -EFAULT;
	if (request.timeout_ms > VE_LAB_MAX_WAIT_MS)
		return -EINVAL;

	left = wait_event_interruptible_timeout(lab->wait,
						lab->came || lab->unknown,
						msecs_to_jiffies(request.timeout_ms));
	if (left < 0)
		return left;

	spin_lock_irqsave(&lab->irq_lock, flags);
	request.came = lab->came;
	request.status = lab->status;
	request.unknown = lab->unknown;
	lab->came = false;
	lab->status = 0;
	spin_unlock_irqrestore(&lab->irq_lock, flags);

	if (copy_to_user(argument, &request, sizeof(request)))
		return -EFAULT;
	return 0;
}

static long ve_lab_ioctl(struct file *file, unsigned int command,
			 unsigned long argument)
{
	struct ve_lab *lab = ve_lab_of(file);
	void __user *user = (void __user *)argument;
	struct ve_lab_info info;
	long ret;

	/* The wait must not hold what a second thread would need to end it. */
	if (command == VE_LAB_IOC_WAIT)
		return ve_lab_wait(lab, user);

	mutex_lock(&lab->lock);
	switch (command) {
	case VE_LAB_IOC_INFO:
		info.abi = VE_LAB_ABI;
		info.version = lab->version;
		info.window_bytes = lab->hw.size;
		info.max_buffers = VE_LAB_MAX_BUFFERS;
		ret = copy_to_user(user, &info, sizeof(info)) ? -EFAULT : 0;
		break;
	case VE_LAB_IOC_ALLOC:
		ret = ve_lab_alloc(lab, user);
		break;
	case VE_LAB_IOC_RESET:
		ret = ve_hw_reset(&lab->hw);
		break;
	default:
		ret = -ENOTTY;
		break;
	}
	mutex_unlock(&lab->lock);
	return ret;
}

static int ve_lab_mmap(struct file *file, struct vm_area_struct *vma)
{
	struct ve_lab *lab = ve_lab_of(file);
	size_t length = vma->vm_end - vma->vm_start;
	unsigned long slot = vma->vm_pgoff >> (VE_LAB_BUFFER_SHIFT - PAGE_SHIFT);
	struct ve_lab_buffer *buffer;
	int ret;

	if (!(vma->vm_flags & VM_SHARED))
		return -EINVAL;
	if (vma->vm_pgoff & ((1UL << (VE_LAB_BUFFER_SHIFT - PAGE_SHIFT)) - 1))
		return -EINVAL;

	if (slot == 0) {
		if (length != PAGE_ALIGN(lab->hw.size))
			return -EINVAL;
		vma->vm_page_prot = pgprot_noncached(vma->vm_page_prot);
		vma->vm_flags |= VM_IO | VM_DONTEXPAND | VM_DONTDUMP;
		return io_remap_pfn_range(vma, vma->vm_start,
					  lab->hw.phys >> PAGE_SHIFT, length,
					  vma->vm_page_prot);
	}
	if (slot > VE_LAB_MAX_BUFFERS)
		return -EINVAL;

	mutex_lock(&lab->lock);
	buffer = &lab->buffers[slot - 1];
	if (!buffer->cpu || length != buffer->size) {
		ret = -EINVAL;
	} else {
		vma->vm_pgoff = 0;
		ret = dma_mmap_coherent(lab->hw.dev, vma, buffer->cpu,
					buffer->dma, buffer->size);
	}
	mutex_unlock(&lab->lock);
	return ret;
}

static const struct file_operations ve_lab_fops = {
	.owner = THIS_MODULE,
	.open = ve_lab_open,
	.release = ve_lab_release,
	.unlocked_ioctl = ve_lab_ioctl,
	.mmap = ve_lab_mmap,
	.llseek = noop_llseek,
};

static int ve_lab_probe(struct platform_device *pdev)
{
	struct ve_lab *lab;
	int ret;

	lab = devm_kzalloc(&pdev->dev, sizeof(*lab), GFP_KERNEL);
	if (!lab)
		return -ENOMEM;
	mutex_init(&lab->lock);
	spin_lock_init(&lab->irq_lock);
	init_waitqueue_head(&lab->wait);

	ret = ve_hw_probe(pdev, &lab->hw, ve_lab_irq, lab, mod_rate);
	if (ret)
		return ret;

	lab->misc.minor = MISC_DYNAMIC_MINOR;
	lab->misc.name = "hardened-blikvm-ve-lab";
	lab->misc.fops = &ve_lab_fops;
	lab->misc.mode = 0600;
	lab->misc.parent = &pdev->dev;
	ret = misc_register(&lab->misc);
	if (ret) {
		ve_hw_remove(&lab->hw);
		return ret;
	}

	platform_set_drvdata(pdev, lab);
	dev_info(&pdev->dev, "lab module bound; the engine is a root process's\n");
	return 0;
}

static int ve_lab_remove(struct platform_device *pdev)
{
	struct ve_lab *lab = platform_get_drvdata(pdev);

	misc_deregister(&lab->misc);
	ve_hw_remove(&lab->hw);
	return 0;
}

static const struct of_device_id ve_lab_of_match[] = {
	{ .compatible = "allwinner,sun50i-h616-video-engine" },
	{ }
};
MODULE_DEVICE_TABLE(of, ve_lab_of_match);

static struct platform_driver ve_lab_driver = {
	.probe = ve_lab_probe,
	.remove = ve_lab_remove,
	.driver = {
		.name = "hardened-blikvm-ve-lab",
		.of_match_table = ve_lab_of_match,
		/* An open device must not lose its engine to an unbind. */
		.suppress_bind_attrs = true,
	},
};
module_platform_driver(ve_lab_driver);

MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Lab access to the H616 video engine for Hardened BliKVM; never packaged");
