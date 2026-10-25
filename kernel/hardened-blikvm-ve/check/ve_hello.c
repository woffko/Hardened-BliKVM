// SPDX-License-Identifier: GPL-2.0-only
/* An empty module: does the build recipe make something this kernel loads? */
#include <linux/init.h>
#include <linux/module.h>

static int __init ve_hello_init(void)
{
	pr_info("ve_hello: loaded\n");
	return 0;
}

static void __exit ve_hello_exit(void)
{
	pr_info("ve_hello: unloaded\n");
}

module_init(ve_hello_init);
module_exit(ve_hello_exit);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("Build recipe check for the Hardened BliKVM video engine modules");
