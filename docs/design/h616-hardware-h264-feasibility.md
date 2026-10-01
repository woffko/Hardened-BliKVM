# Hardware H.264 encoding on the H616: feasibility (2026-10-01)

Question: can the H616's video engine encode the WebRTC stream instead of
x264 on the CPU? A study of at most one working day, from open sources only:
the GPL cedar driver of Allwinner's BSP kernel, linux-sunxi documentation,
Bootlin's cedrus work, the mainline kernel and its mailing lists. No
proprietary library was disassembled or decompiled, and on the board only
read-only checks were made (no module loaded, no register touched).

## What the hardware has

- Allwinner's H616 brief lists H.264 encoding (BP, MP and HP profiles, up to
  4K at 25 frames/s, CBR and VBR) next to H.265, VP9, H.264 and other
  decoders. The vendor's Android uses it through its closed CedarX libraries.
- The board's video engine is the device-tree node
  `allwinner,sun50i-h616-video-engine` at `0x1c0e000` (clocks `ahb`, `mod`,
  `ram`; an SRAM reference; no `iommus`). The board's kernel (Armbian
  5.19.4) drives it with `cedrus` as a decoder only: its output queue takes
  MPEG-2, H.264 and HEVC slices and VP8 frames, its capture queue gives NV12
  and the 32x32-tiled ST12. There is no IOMMU node in this device tree.

## Open-source state

- Decoding: mainline `cedrus` gains the H616 in a 2026 series by Chen-Yu
  Tsai (reviewed and acked, July 2026): the H616 reuses the H6 variant
  (MPEG-2, H.264, HEVC including 10-bit, VP8; VP9 is left for later) and its
  node gets two IOMMU ports and two SRAM regions. The H616 IOMMU driver is in
  mainline since 6.11 (Andre Przywara). The board's 5.19 kernel carries an
  older Armbian patch without the IOMMU.
- Encoding: no mainline driver for any Allwinner SoC. Bootlin implemented
  stateless H.264 encoding in `cedrus` for the V3/V3s/S3 only (branch
  `cedrus/h264-encoding` of `bootlin/linux`, November 2023; the encoder is
  about 1600 lines; `bootlin/v4l2-cedrus-enc-test` is the only user). It
  supports I and P frames and CABAC, but has no rate control (QP only), no
  scaling or format conversion, and builds the bitstream headers in the
  driver. It is not proposed for mainline because V4L2 has no uAPI for
  stateless encoders yet. In that branch only the V3s variant has the
  encoder capability; Bootlin describe the other generations' encoders as
  "different (yet rather similar)". An attempt to use it on the H616 (the
  test tool's issue #1, January 2024) never got past opening the device.
- Older reverse-engineered encoders (jemk's `h264enc`, the cedrus264 FFmpeg
  patch for the H3) used the vendor's `/dev/cedar_dev` interface on A10-H3
  generations; linux-sunxi documents that encoder's registers (block at
  `0xb00`) for those generations and lists VE versions up to the H5
  (`0x1718`) and S3 (`0x1681`); the H6 and H616 versions are not listed.
- The vendor's encoder library (`libvencoder.so` in `libcedarc`) is binary
  only; the public repositories carry its API header and an OpenMAX wrapper,
  no register description, and no license file.

## What the open BSP driver shows about the H616 encoder

The BSP kernel's `drivers/media/cedar-ve/cedar_ve.c` (Orange Pi's
`orange-pi-4.9-sun50iw9` branch, GPL) is the kernel half of the vendor
stack:

- it maps the whole register window (`0x01c0e000`) into the calling process
  (`io_remap_pfn_range`, uncached), so the closed library programs the
  engine directly and the kernel only gates clocks, takes interrupts and maps
  buffers;
- on every generation except sun3i, including the H616 (sun50iw9), its
  interrupt handler finds the H.264 encoder by bit 7 of the engine select
  register and reads the encoder's interrupt status and control at `0xb1c`
  and `0xb14`, the offsets linux-sunxi documents for the older generations
  and that Bootlin's V3 encoder uses. The block's base and its control
  registers therefore stay where they were; whether the rest of the block
  (picture parameters, reference and reconstruction buffers, input formats,
  motion estimation) matches the V3's is not shown by any open source;
- buffers are handed to the engine as dma-bufs through the IOMMU
  (`IOCTL_GET_IOMMU_ADDR`);
- a separate block at `0xe00`, selected by bit 5 and enabled for sun50i
  parts, raises JPEG decoder interrupts (`IOCTL_WAIT_JPEG_DEC`). No open
  driver for it is known.

## Unknown

- The H616 engine's version value and how far its encoder differs from the
  V3's beyond the control registers above.
- The input formats the encoder takes on this generation (tiled or linear
  NV12) and its buffer and SRAM requirements.
- Anything about the JPEG decoder block.

Each of these needs either documentation that is not public or observing
the vendor stack at work on a board, which this study did not do.

## Security

Only a kernel driver that owns the buffers is acceptable here. The engine is
a DMA master: a process that can program it can read and write any memory
the engine can reach, which is what the vendor stack gives its user-space
library. With the H616 IOMMU (mainline 6.11 and the 2026 video-engine
series) a V4L2 driver can confine the engine to the buffers it maps; the
board's current kernel has neither, so a kernel upgrade comes first.

## What a usable path would need

1. A board kernel with the H616 IOMMU and the mainline H616 video-engine
   node (6.11 or later plus the 2026 series), as a new SD image; a project of
   its own.
2. Bootlin's encoder ported to that kernel and extended with an H616
   variant, then debugged against the unknown register differences.
3. A private V4L2 interface for it (no upstream uAPI), and in the encoder a
   V4L2 client with software rate control in place of x264.
4. For the larger gain, hardware JPEG decoding too, with no open starting
   point.

Estimate: steps 1-3 are weeks to months of kernel work with uncertain
success, carried out of tree and redone on every kernel update; step 4 is a
separate research project.

## Benefit

With libjpeg-turbo, preparing a 1920x1080 frame takes about 25 ms on two
converter threads and x264 ultrafast encodes it in 28 ms, so H.264 at
1920x1080 reaches about 36 frames/s, more than the capture's 30
(`h616-software-video-encoder.md`). A hardware encoder would free about one
core while a WebRTC viewer watches and shorten a frame's delay by about
20 ms; the frame rate is already capture-bound.

## Recommendation

No-go for now. Revisit when (a) the board moves to a kernel with the H616
IOMMU and video-engine support, and (b) either V4L2 gains a stateless
encoding uAPI or the CPU budget becomes the limit (for example several
WebRTC viewers at once). The first step then is cheap and read-only: read
the engine's version from a running mainline `cedrus` and compare the V3
encoder's register use against what the engine reports.

## Sources

- Allwinner H616 brief: https://www.allwinnertech.com/uploads/pdf/20200409170709ca.pdf
- linux-sunxi, VE Register guide, Sunxi-Cedrus and H616 pages (archived
  copies, 2025)
- H616 video engine series (Chen-Yu Tsai, July 2026):
  https://ratatoskr.run/linux-arm-kernel/2026/07/17247254/t
- H616 IOMMU (Andre Przywara):
  https://patchwork.ozlabs.org/project/devicetree-bindings/cover/20240616224056.29159-1-andre.przywara@arm.com/
- Bootlin, V3/V3s/S3 H.264 encoder (November 2023):
  https://bootlin.com/blog/open-source-linux-kernel-support-for-the-allwinner-v3-v3s-s3-h-264-video-encoder/
  and the `cedrus/h264-encoding` branch of https://github.com/bootlin/linux
- H616 attempt: https://github.com/bootlin/v4l2-cedrus-enc-test/issues/1
- BSP cedar driver: `drivers/media/cedar-ve/cedar_ve.c` in
  https://github.com/orangepi-xunlong/linux-orangepi (branch
  `orange-pi-4.9-sun50iw9`)
- libcedarc (unofficial mirror): https://github.com/aodzip/libcedarc
