# Kernel modules

Modules for the board's kernel, built by `scripts/build-ve-module.sh`. They
are works of their own beside the application: a kernel module is licensed
GPL-2.0-only, as the kernel requires, and every file here says so
(`docs/licensing.md`).

- `hardened-blikvm-ve/check`: two modules without a purpose of their own.
  They prove on the board that the build recipe makes modules this kernel
  loads and that its structures are the running kernel's.
- `hardened-blikvm-ve`: the video engine's modules of the hardware H.264
  plan (`docs/baselines/work-plan-20261002-hardware-h264.md`).
