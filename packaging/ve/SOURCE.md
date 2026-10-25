# The encoder driver of the H616's video engine: its source

These files are the source of `hardened-blikvm-ve-enc.ko` as this package
carries it. They are licensed GPL-2.0-only, as each of them says; on a
Debian system the licence's text is `/usr/share/common-licenses/GPL-2`.

The module is built for one kernel release and loads on no other. For a
kernel's build tree of exactly that release, a Makefile beside the files
with

    obj-m += hardened-blikvm-ve-enc.o
    hardened-blikvm-ve-enc-y := ve_enc.o ve_enc_plan.o ve_hw.o

builds it. The project's repository has the recipe that puts such a tree
together for the board's kernel (`scripts/build-ve-module.sh`), the driver's
design and threat model (`docs/design/h616-ve-encoder-driver-v1.md`) and its
tests (`kernel/hardened-blikvm-ve/test`).
