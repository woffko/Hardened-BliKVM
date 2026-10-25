// SPDX-License-Identifier: GPL-2.0-only
/*
 * A module that registers what the video engine modules register, without
 * touching the engine: a platform driver that matches nothing on this board,
 * and a device node with a request and a mapping. If the headers of the build
 * did not fit the running kernel, these structures would not either.
 */
#include <linux/fs.h>
#include <linux/init.h>
#include <linux/miscdevice.h>
#include <linux/mm.h>
#include <linux/module.h>
#include <linux/mod_devicetable.h>
#include <linux/of.h>
#include <linux/platform_device.h>
#include <linux/slab.h>
#include <linux/uaccess.h>

static void *ve_probe_page;

static long ve_probe_ioctl(struct file *file, unsigned int cmd, unsigned long arg)
{
	u32 answer = 0x48363136; /* "H616" */

	if (cmd != 0x5601)
		return -ENOTTY;
	if (copy_to_user((void __user *)arg, &answer, sizeof(answer)))
		return -EFAULT;
	return 0;
}

static int ve_probe_mmap(struct file *file, struct vm_area_struct *vma)
{
	if (vma->vm_end - vma->vm_start != PAGE_SIZE || vma->vm_pgoff)
		return -EINVAL;
	return remap_pfn_range(vma, vma->vm_start,
			       virt_to_phys(ve_probe_page) >> PAGE_SHIFT,
			       PAGE_SIZE, vma->vm_page_prot);
}

static const struct file_operations ve_probe_fops = {
	.owner = THIS_MODULE,
	.unlocked_ioctl = ve_probe_ioctl,
	.mmap = ve_probe_mmap,
};

static struct miscdevice ve_probe_misc = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = "hardened-blikvm-ve-probe-test",
	.fops = &ve_probe_fops,
	.mode = 0600,
};

static int ve_probe_probe(struct platform_device *pdev)
{
	return -ENODEV;
}

static const struct of_device_id ve_probe_of_match[] = {
	{ .compatible = "hardened-blikvm,nothing-on-this-board" },
	{ }
};
MODULE_DEVICE_TABLE(of, ve_probe_of_match);

static struct platform_driver ve_probe_driver = {
	.probe = ve_probe_probe,
	.driver = {
		.name = "hardened-blikvm-ve-probe-test",
		.of_match_table = ve_probe_of_match,
	},
};

static int __init ve_probe_init(void)
{
	int ret;

	ve_probe_page = (void *)get_zeroed_page(GFP_KERNEL);
	if (!ve_probe_page)
		return -ENOMEM;
	memcpy(ve_probe_page, "hardened-blikvm-ve", 18);
	ret = misc_register(&ve_probe_misc);
	if (ret)
		goto free_page;
	ret = platform_driver_register(&ve_probe_driver);
	if (ret)
		goto deregister;
	pr_info("ve_probe_test: loaded\n");
	return 0;

deregister:
	misc_deregister(&ve_probe_misc);
free_page:
	free_page((unsigned long)ve_probe_page);
	return ret;
}

static void __exit ve_probe_exit(void)
{
	platform_driver_unregister(&ve_probe_driver);
	misc_deregister(&ve_probe_misc);
	free_page((unsigned long)ve_probe_page);
	pr_info("ve_probe_test: unloaded\n");
}

module_init(ve_probe_init);
module_exit(ve_probe_exit);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Structure check for the Hardened BliKVM video engine modules");
