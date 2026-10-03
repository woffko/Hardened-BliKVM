// SPDX-License-Identifier: GPL-2.0-only
/*
 * The encoder driver of Hardened BliKVM for the H616's video engine
 * (docs/design/h616-ve-encoder-driver-v1.md). It gives one client the
 * engine's H.264 encoder and nothing else of the engine: the client says
 * which picture is to be encoded at which QP and gives the bits of the
 * frame's headers; every register and every address is the driver's.
 *
 * The engine has no IOMMU in front of it on this board. Whatever stands in
 * its address registers when an encode starts is where it reads and
 * writes. So the addresses come from this file's own table of buffers and
 * from nowhere else, the plan that turns a request into register
 * operations is a plain function that is tested with random requests on
 * the build host (ve_enc_plan.c), and the engine is reset before a buffer
 * is freed.
 */
#include <linux/delay.h>
#include <linux/dma-mapping.h>
#include <linux/fs.h>
#include <linux/iopoll.h>
#include <linux/ktime.h>
#include <linux/miscdevice.h>
#include <linux/module.h>
#include <linux/mod_devicetable.h>
#include <linux/mutex.h>
#include <linux/of.h>
#include <linux/slab.h>
#include <linux/spinlock.h>
#include <linux/uaccess.h>
#include <linux/wait.h>

#include "ve_enc_abi.h"
#include "ve_enc_plan.h"
#include "ve_hw.h"

/* The rate the decoder driver gives the engine's clock on this SoC. */
static unsigned long mod_rate = 600000000;
module_param(mod_rate, ulong, 0444);
MODULE_PARM_DESC(mod_rate, "rate of the engine's module clock in Hz");

/* A 1920x1080 frame takes the engine 7 ms. */
#define VE_ENC_TIMEOUT_MS	200
/* The bit writer answers at once; this is how long it is waited for. */
#define VE_ENC_PUT_TIMEOUT_US	1000
#define VE_ENC_REG_DECODER_IP	0x0e0
#define VE_ENC_REG_ENCODER_IP	0x0e4
#define VE_ENC_REG_STATISTICS	0xb50
#define VE_ENC_STATUS_FINISH	BIT(0)

/* The interface's structures are what its client was built with. */
static_assert(sizeof(struct ve_enc_info) == 32);
static_assert(sizeof(struct ve_enc_config) == 24);
static_assert(sizeof(struct ve_enc_unit) == 132);
static_assert(sizeof(struct ve_enc_frame) == 72);

/* Memory the engine is given: coherent, or pages the processor caches. */
struct ve_enc_buffer {
	void *cpu;
	struct page *pages;
	dma_addr_t dma;
	size_t size;
	enum dma_data_direction direction;
};

enum {
	BUFFER_INPUT,
	BUFFER_STREAM,
	/* VE_ENC_SETS of each. */
	BUFFER_REC,
	BUFFER_SUBPIXEL = BUFFER_REC + VE_ENC_SETS,
	BUFFER_MB_INFO = BUFFER_SUBPIXEL + VE_ENC_SETS,
	BUFFER_COUNT,
};

struct ve_enc {
	struct ve_hw hw;
	struct miscdevice misc;
	/* Open and release, and every request from beginning to end. */
	struct mutex lock;
	unsigned long opened;

	/* A stream, between a configure and the next or the close. */
	bool configured;
	struct ve_enc_geometry geometry;
	struct ve_enc_addresses addresses;
	struct ve_enc_buffer buffers[BUFFER_COUNT];
	/* Which sets of reconstruction buffers hold the passes made so far. */
	struct ve_enc_sets sets;
	/* A whole picture is in the input buffer. */
	bool have_picture;
	/* The bytes of the stream buffer the processor has read. */
	size_t stream_read;

	u32 decoder_ip;
	u32 encoder_ip;

	/* The interrupt's news. */
	spinlock_t irq_lock;
	wait_queue_head_t wait;
	bool came;
	bool unknown;
	bool line_off;
	u32 status;

	/* Too large for a stack. */
	struct ve_enc_unit units[VE_ENC_MAX_UNITS];
	struct ve_enc_op ops[VE_ENC_MAX_OPS];
};

static struct ve_enc *ve_enc_of(struct file *file)
{
	return container_of(file->private_data, struct ve_enc, misc);
}

/*
 * The encoder's interrupt, as the vendor kernel's driver handles it: with
 * the encoder selected and a status bit and an enable bit set, the enables
 * are cleared and the waiter is woken. Anything else cannot be quieted from
 * here: the line is switched off until the next frame's reset.
 */
static irqreturn_t ve_enc_irq(int irq, void *data)
{
	struct ve_enc *enc = data;
	struct ve_hw *hw = &enc->hw;
	unsigned long flags;
	bool ours = false;
	u32 status;

	spin_lock_irqsave(&enc->irq_lock, flags);
	if (hw->powered && (ve_read(hw, VE_MODE) & VE_MODE_ENCODER)) {
		status = ve_read(hw, VE_AVC_STATUS) & VE_AVC_STATUS_MASK;
		if (status && (ve_read(hw, VE_AVC_INT_EN) & VE_AVC_INT_EN_MASK)) {
			ve_write(hw, VE_AVC_INT_EN,
				 ve_read(hw, VE_AVC_INT_EN) & ~VE_AVC_INT_EN_MASK);
			enc->status = status;
			enc->came = true;
			ours = true;
		}
	}
	if (!ours) {
		disable_irq_nosync(irq);
		enc->line_off = true;
		enc->unknown = true;
	}
	spin_unlock_irqrestore(&enc->irq_lock, flags);

	wake_up(&enc->wait);
	return IRQ_HANDLED;
}

/*
 * The engine back to its state after power-on, nothing pending, the line
 * on. Whatever the engine was doing with the buffers has stopped when this
 * returns. Called with the lock held and the engine powered.
 */
static void ve_enc_reset(struct ve_enc *enc)
{
	unsigned long flags;
	bool line_off;

	/* No handler reads a register across the reset. */
	disable_irq(enc->hw.irq);
	ve_hw_reset(&enc->hw);
	spin_lock_irqsave(&enc->irq_lock, flags);
	line_off = enc->line_off;
	enc->line_off = false;
	enc->came = false;
	enc->unknown = false;
	enc->status = 0;
	spin_unlock_irqrestore(&enc->irq_lock, flags);
	/* Once for the handler's own switch-off, once for the one above. */
	if (line_off)
		enable_irq(enc->hw.irq);
	enable_irq(enc->hw.irq);
}

static void ve_enc_free_buffers(struct ve_enc *enc)
{
	unsigned int index;

	for (index = 0; index < BUFFER_COUNT; index++) {
		struct ve_enc_buffer *buffer = &enc->buffers[index];

		if (buffer->pages)
			dma_free_pages(enc->hw.dev, buffer->size, buffer->pages,
				       buffer->dma, buffer->direction);
		else if (buffer->cpu)
			dma_free_coherent(enc->hw.dev, buffer->size, buffer->cpu,
					  buffer->dma);
		memset(buffer, 0, sizeof(*buffer));
	}
	enc->configured = false;
	ve_enc_sets_reset(&enc->sets);
	enc->have_picture = false;
	enc->stream_read = 0;
}

/*
 * One buffer of `size` bytes, zeroed, below 4 GiB and at a multiple of a
 * page. With a direction: pages the processor caches, handed to and fro.
 */
static int ve_enc_alloc(struct ve_enc *enc, unsigned int index, size_t size,
			enum dma_data_direction direction)
{
	struct ve_enc_buffer *buffer = &enc->buffers[index];
	struct device *dev = enc->hw.dev;

	/*
	 * Memory that is not there is the client's answer (ENOMEM), not a
	 * report in the kernel's log: a client can ask for it as often as
	 * it likes.
	 */
	if (direction == DMA_NONE) {
		buffer->cpu = dma_alloc_coherent(dev, size, &buffer->dma,
						 GFP_KERNEL | __GFP_NOWARN);
	} else {
		buffer->pages = dma_alloc_pages(dev, size, &buffer->dma,
						direction,
						GFP_KERNEL | __GFP_NOWARN);
		if (buffer->pages) {
			buffer->cpu = page_address(buffer->pages);
			memset(buffer->cpu, 0, size);
			dma_sync_single_for_device(dev, buffer->dma, size,
						   direction);
		}
	}
	if (!buffer->cpu)
		return -ENOMEM;
	buffer->size = size;
	buffer->direction = direction;
	/* What the plan's address forms and the engine's registers can hold. */
	if (upper_32_bits(buffer->dma) ||
	    upper_32_bits((u64)buffer->dma + size) ||
	    (lower_32_bits(buffer->dma) & (VE_ENC_PAGE - 1)))
		return -ENXIO;
	return 0;
}

static long ve_enc_configure(struct ve_enc *enc, void __user *argument)
{
	struct ve_enc_config config;
	struct ve_enc_geometry geometry;
	unsigned int set;
	int ret;

	if (copy_from_user(&config, argument, sizeof(config)))
		return -EFAULT;
	if (config.reserved ||
	    ve_enc_geometry(config.width, config.height, config.flags, &geometry))
		return -EINVAL;
	if (ve_enc_total_bytes(&geometry) > VE_ENC_MAX_TOTAL_BYTES)
		return -EINVAL;

	/* The engine lets go of the old buffers before they are freed. */
	ve_enc_reset(enc);
	ve_enc_free_buffers(enc);

	ret = ve_enc_alloc(enc, BUFFER_INPUT, geometry.input_bytes, DMA_TO_DEVICE);
	if (!ret)
		ret = ve_enc_alloc(enc, BUFFER_STREAM, geometry.stream_bytes,
				   DMA_FROM_DEVICE);
	for (set = 0; !ret && set < VE_ENC_SETS; set++) {
		ret = ve_enc_alloc(enc, BUFFER_REC + set, geometry.rec_bytes,
				   DMA_NONE);
		if (!ret)
			ret = ve_enc_alloc(enc, BUFFER_SUBPIXEL + set,
					   geometry.subpixel_bytes, DMA_NONE);
	}
	if (!ret)
		ret = ve_enc_alloc(enc, BUFFER_MB_INFO, geometry.mb_info_bytes,
				   DMA_NONE);
	if (ret) {
		ve_enc_free_buffers(enc);
		return ret;
	}

	enc->geometry = geometry;
	enc->addresses.input = lower_32_bits(enc->buffers[BUFFER_INPUT].dma);
	enc->addresses.stream = lower_32_bits(enc->buffers[BUFFER_STREAM].dma);
	for (set = 0; set < VE_ENC_SETS; set++) {
		enc->addresses.rec[set] =
			lower_32_bits(enc->buffers[BUFFER_REC + set].dma);
		enc->addresses.subpixel[set] =
			lower_32_bits(enc->buffers[BUFFER_SUBPIXEL + set].dma);
	}
	enc->addresses.mb_info = lower_32_bits(enc->buffers[BUFFER_MB_INFO].dma);
	enc->configured = true;

	config.picture_bytes = geometry.picture_bytes;
	config.stream_bytes = geometry.stream_bytes;
	if (copy_to_user(argument, &config, sizeof(config))) {
		ve_enc_free_buffers(enc);
		return -EFAULT;
	}
	return 0;
}

/* Carries the plan out up to its start. 0, or -EIO if the bit writer is not ready. */
static int ve_enc_run(struct ve_enc *enc, unsigned int count)
{
	struct ve_hw *hw = &enc->hw;
	unsigned int at;
	u32 value;

	for (at = 0; at < count; at++) {
		const struct ve_enc_op *op = &enc->ops[at];

		switch (op->kind) {
		case VE_ENC_OP_WRITE:
			ve_write(hw, op->reg, op->value);
			break;
		case VE_ENC_OP_UPDATE:
			value = ve_read(hw, op->reg);
			ve_write(hw, op->reg, (value & ~op->clear) | op->value);
			break;
		case VE_ENC_OP_PUT:
			if (readl_poll_timeout_atomic(hw->base + VE_ENC_REG_STATUS,
						      value,
						      value & VE_ENC_STATUS_PUT_READY,
						      1, VE_ENC_PUT_TIMEOUT_US))
				return -EIO;
			ve_write(hw, VE_ENC_REG_PUT_DATA, op->value);
			ve_write(hw, VE_ENC_REG_TRIGGER,
				 VE_ENC_TRIGGER_PUT | (u32)op->bits << 8);
			break;
		case VE_ENC_OP_START:
			ve_write(hw, VE_ENC_REG_TRIGGER, VE_ENC_TRIGGER_START);
			break;
		}
	}
	return 0;
}

static long ve_enc_encode(struct ve_enc *enc, void __user *argument)
{
	struct ve_enc_buffer *input = &enc->buffers[BUFFER_INPUT];
	struct ve_enc_buffer *stream = &enc->buffers[BUFFER_STREAM];
	struct device *dev = enc->hw.dev;
	struct ve_enc_request request;
	struct ve_enc_frame frame;
	struct ve_enc_pass pass;
	unsigned long flags;
	unsigned int count;
	ktime_t started;
	u32 bits;
	int ret;

	if (copy_from_user(&frame, argument, sizeof(frame)))
		return -EFAULT;
	if (!enc->configured || frame.reserved)
		return -EINVAL;
	if (frame.unit_count < 1 || frame.unit_count > VE_ENC_MAX_UNITS)
		return -EINVAL;
	if (copy_from_user(enc->units, u64_to_user_ptr(frame.units),
			   frame.unit_count * sizeof(enc->units[0])))
		return -EFAULT;
	request.flags = frame.flags;
	request.qp = frame.qp;
	request.unit_count = frame.unit_count;
	request.units = enc->units;
	ret = ve_enc_check_frame(&request, &enc->sets, enc->have_picture, &pass);
	if (ret)
		return ret;

	if (!(frame.flags & VE_ENC_AGAIN)) {
		if (frame.picture_bytes != enc->geometry.picture_bytes)
			return -EINVAL;
		/* Until the picture is whole, there is none to encode again. */
		enc->have_picture = false;
		dma_sync_single_for_cpu(dev, input->dma, input->size, DMA_TO_DEVICE);
		if (copy_from_user(input->cpu, u64_to_user_ptr(frame.picture),
				   frame.picture_bytes)) {
			ve_enc_sets_reset(&enc->sets);
			return -EFAULT;
		}
		dma_sync_single_for_device(dev, input->dma, input->size,
					   DMA_TO_DEVICE);
		enc->have_picture = true;
	}

	count = ve_enc_plan(&enc->geometry, &enc->addresses, &request, &pass,
			    enc->ops, VE_ENC_MAX_OPS);
	if (!count)
		return -EINVAL;

	ve_enc_reset(enc);
	/* What the processor read of the last frame goes back to the device. */
	if (enc->stream_read)
		dma_sync_single_for_device(dev, stream->dma, enc->stream_read,
					   DMA_FROM_DEVICE);
	enc->stream_read = 0;

	started = ktime_get();
	ret = ve_enc_run(enc, count);
	if (ret)
		goto failed;

	/*
	 * Not interruptible: a frame takes the engine 7 ms, and a client's
	 * signal is no reason to reset an engine in the middle of its work
	 * on memory. The wait is bounded, so the signal is taken soon.
	 */
	wait_event_timeout(enc->wait, enc->came || enc->unknown,
			   msecs_to_jiffies(VE_ENC_TIMEOUT_MS));
	frame.encode_us = (u32)ktime_us_delta(ktime_get(), started);
	spin_lock_irqsave(&enc->irq_lock, flags);
	frame.status = enc->status;
	if (enc->unknown)
		ret = -EIO;
	else if (!enc->came)
		ret = -ETIMEDOUT;
	else if (enc->status != VE_ENC_STATUS_FINISH)
		ret = -EIO;
	spin_unlock_irqrestore(&enc->irq_lock, flags);
	if (ret)
		goto failed;

	bits = ve_read(&enc->hw, VE_ENC_REG_LENGTH);
	frame.statistics[0] = ve_read(&enc->hw, VE_ENC_REG_STATISTICS);
	frame.statistics[1] = ve_read(&enc->hw, VE_ENC_REG_STATISTICS + 4);
	frame.statistics[2] = ve_read(&enc->hw, VE_ENC_REG_STATISTICS + 8);
	if (!bits || (bits & 7) || bits / 8 > stream->size) {
		ret = -EIO;
		goto failed;
	}
	frame.stream_bytes = bits / 8;
	/* The pass is a frame: a picture may refer to it. */
	ve_enc_sets_done(&enc->sets, frame.flags, &pass);

	if (frame.stream_bytes > frame.stream_capacity) {
		ret = -ENOSPC;
	} else {
		enc->stream_read = frame.stream_bytes;
		dma_sync_single_for_cpu(dev, stream->dma, frame.stream_bytes,
					DMA_FROM_DEVICE);
		if (copy_to_user(u64_to_user_ptr(frame.stream), stream->cpu,
				 frame.stream_bytes))
			ret = -EFAULT;
	}
	if (copy_to_user(argument, &frame, sizeof(frame)))
		return -EFAULT;
	return ret;

failed:
	/*
	 * The engine must not go on with the buffers after a failure, and
	 * what it left in them is no frame: the stream begins again with a
	 * key frame.
	 */
	ve_enc_reset(enc);
	ve_enc_sets_reset(&enc->sets);
	frame.stream_bytes = 0;
	if (copy_to_user(argument, &frame, sizeof(frame)))
		return -EFAULT;
	return ret;
}

static int ve_enc_open(struct inode *inode, struct file *file)
{
	struct ve_enc *enc = ve_enc_of(file);
	int ret;

	if (test_and_set_bit(0, &enc->opened))
		return -EBUSY;

	mutex_lock(&enc->lock);
	ret = ve_hw_power_on(&enc->hw);
	if (!ret) {
		enc->decoder_ip = ve_read(&enc->hw, VE_ENC_REG_DECODER_IP);
		enc->encoder_ip = ve_read(&enc->hw, VE_ENC_REG_ENCODER_IP);
	}
	mutex_unlock(&enc->lock);
	if (ret)
		clear_bit(0, &enc->opened);
	return ret;
}

static int ve_enc_release(struct inode *inode, struct file *file)
{
	struct ve_enc *enc = ve_enc_of(file);
	unsigned long flags;
	bool line_off;

	mutex_lock(&enc->lock);
	/*
	 * No handler may read a register while the clocks go. The reset
	 * stops whatever the engine was doing with the buffers; only then
	 * are they freed.
	 */
	disable_irq(enc->hw.irq);
	ve_hw_power_off(&enc->hw);
	ve_enc_free_buffers(enc);

	spin_lock_irqsave(&enc->irq_lock, flags);
	line_off = enc->line_off;
	enc->line_off = false;
	enc->came = false;
	enc->unknown = false;
	spin_unlock_irqrestore(&enc->irq_lock, flags);
	if (line_off)
		enable_irq(enc->hw.irq);
	enable_irq(enc->hw.irq);
	mutex_unlock(&enc->lock);

	clear_bit(0, &enc->opened);
	return 0;
}

static long ve_enc_ioctl(struct file *file, unsigned int command,
			 unsigned long argument)
{
	struct ve_enc *enc = ve_enc_of(file);
	void __user *user = (void __user *)argument;
	struct ve_enc_info info;
	long ret;

	if (mutex_lock_interruptible(&enc->lock))
		return -EINTR;
	switch (command) {
	case VE_ENC_IOC_INFO:
		memset(&info, 0, sizeof(info));
		info.abi = VE_ENC_ABI;
		info.decoder_ip = enc->decoder_ip;
		info.encoder_ip = enc->encoder_ip;
		info.max_width = VE_ENC_MAX_WIDTH;
		info.max_height = VE_ENC_MAX_HEIGHT;
		info.min_width = VE_ENC_MIN_WIDTH;
		info.min_height = VE_ENC_MIN_HEIGHT;
		ret = copy_to_user(user, &info, sizeof(info)) ? -EFAULT : 0;
		break;
	case VE_ENC_IOC_CONFIGURE:
		ret = ve_enc_configure(enc, user);
		break;
	case VE_ENC_IOC_ENCODE:
		ret = ve_enc_encode(enc, user);
		break;
	default:
		ret = -ENOTTY;
		break;
	}
	mutex_unlock(&enc->lock);
	return ret;
}

static const struct file_operations ve_enc_fops = {
	.owner = THIS_MODULE,
	.open = ve_enc_open,
	.release = ve_enc_release,
	.unlocked_ioctl = ve_enc_ioctl,
	.compat_ioctl = compat_ptr_ioctl,
	.llseek = noop_llseek,
};

static int ve_enc_probe(struct platform_device *pdev)
{
	struct ve_enc *enc;
	int ret;

	enc = devm_kzalloc(&pdev->dev, sizeof(*enc), GFP_KERNEL);
	if (!enc)
		return -ENOMEM;
	mutex_init(&enc->lock);
	spin_lock_init(&enc->irq_lock);
	init_waitqueue_head(&enc->wait);

	ret = ve_hw_probe(pdev, &enc->hw, ve_enc_irq, enc, mod_rate);
	if (ret)
		return ret;

	enc->misc.minor = MISC_DYNAMIC_MINOR;
	enc->misc.name = "hardened-blikvm-ve-enc";
	enc->misc.fops = &ve_enc_fops;
	/* Root's until the system gives the node to the encoder's group. */
	enc->misc.mode = 0600;
	enc->misc.parent = &pdev->dev;
	ret = misc_register(&enc->misc);
	if (ret) {
		ve_hw_remove(&enc->hw);
		return ret;
	}

	platform_set_drvdata(pdev, enc);
	return 0;
}

static int ve_enc_remove(struct platform_device *pdev)
{
	struct ve_enc *enc = platform_get_drvdata(pdev);

	misc_deregister(&enc->misc);
	ve_hw_remove(&enc->hw);
	return 0;
}

static const struct of_device_id ve_enc_of_match[] = {
	{ .compatible = "allwinner,sun50i-h616-video-engine" },
	{ }
};
MODULE_DEVICE_TABLE(of, ve_enc_of_match);

static struct platform_driver ve_enc_driver = {
	.probe = ve_enc_probe,
	.remove = ve_enc_remove,
	.driver = {
		.name = "hardened-blikvm-ve-enc",
		.of_match_table = ve_enc_of_match,
		/* An open device must not lose its engine to an unbind. */
		.suppress_bind_attrs = true,
	},
};
module_platform_driver(ve_enc_driver);

MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("H.264 encoder of the H616 video engine for Hardened BliKVM");
