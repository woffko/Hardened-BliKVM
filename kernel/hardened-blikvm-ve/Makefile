# SPDX-License-Identifier: GPL-2.0-only
# The video engine's modules (docs/design/h616-ve-h264-encoder.md and
# docs/design/h616-ve-encoder-driver-v1.md). The lab module is a lab
# instrument: no package, slot or image carries it. The encoder driver is the
# product's.
ccflags-y += -Werror
obj-m += hardened-blikvm-ve-lab.o
hardened-blikvm-ve-lab-y := ve_lab.o ve_hw.o
obj-m += hardened-blikvm-ve-enc.o
hardened-blikvm-ve-enc-y := ve_enc.o ve_enc_plan.o ve_hw.o
