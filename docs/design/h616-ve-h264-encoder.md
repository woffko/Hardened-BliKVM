# The H616 video engine's H.264 encoder: sources, register maps, lab method

Plan: `docs/baselines/work-plan-20261002-hardware-h264.md`. This note holds
what is known about the encoder from open sources, the two register maps
that are the hypotheses to test, how the lab works, and, as the work goes
on, the lab's record and the register description that comes out of it.

Status (2026-10-02, V0): nothing has been tried on hardware yet.

## Sources

| Source | Licence | Version | What is taken |
| --- | --- | --- | --- |
| `jemk/cedrus`, `h264enc/` | GPL-2.0-or-later | commit `84c5e37` (2015-01-18) | the register sequence of an encode on the A10 to H3 generations, the buffers and their sizes, headers written through the engine's bit writer |
| `jemk/cedrus`, `common/ve.h`, `common/ve.c` | LGPL-2.1-or-later | the same commit | register names and offsets, how an engine is selected |
| `bootlin/linux`, branch `cedrus/h264-encoding`: `cedrus_regs.h`, `cedrus_enc.c`, `cedrus_enc_h264.c` | GPL-2.0 | commit `4e94979` (2023-11-10) | the map of the V3, V3s and S3 with its fields, the encode sequence, the buffers and their sizes, the encoder's reset, the interrupt handling |
| Linux 5.19, `drivers/staging/media/sunxi/cedrus` | GPL-2.0 | v5.19 | what a driver of this engine does at probe: interrupt, SRAM section, clocks, reset, runtime power |
| The vendor kernel's `drivers/media/cedar-ve/cedar_ve.c` (`orangepi-xunlong/linux-orangepi`, branch `orange-pi-4.9-sun50iw9`) | GPL-2.0-or-later | read on 2026-10-02 | which bits of the top register mean encoder, input stage and JPEG decoder on the sun50i parts, where their interrupt status and enables are, and how the handler quiets them |
| linux-sunxi, "VE Register guide" | documentation | archived copy of 2025 | register names of the older generations |
| Armbian forum, topic 26551 (September 2025) | a statement | | that the 32-bit generations' encoder code does not work on the H616 and T507 |
| Allwinner's H616 brief | documentation | | H.264 encoding up to 4K at 25 frames a second |

The first two are read from clones in a cache outside the repository; the
vendor kernel's file was read from its repository. Nothing of them is copied
into this repository. Where code here follows one of them, its file says so
and carries a licence that the source allows.

## Clean room

1. Read and used: sources with an open licence (the table above) and public
   documentation. Register offsets and bit meanings are facts; where code is
   adapted, the file keeps the notice and the licence of its source.
2. Watched, only if the operator approves it (plan, V2R): the vendor's
   encoder library running on the test board as a black box, called through
   its published interface. What is recorded is its traffic with the
   hardware: which register it reads or writes, with which value, in which
   order, for which input. The register description is written from such
   traces in this project's own words.
3. Never: disassembling or decompiling a vendor library; reading vendor
   source that was not published under an open licence (its interface
   headers excepted, as the description of how to call it); copying any of
   it; putting a vendor file into Git, a package, a slot or an image. Such a
   library lives in a cache outside the repository and on the board's tmpfs
   for the length of a session.
4. The product contains only code written here or taken from open sources
   under their licences (`docs/licensing.md`).

## The engine's top level

Offsets are from the engine's base, `0x01c0e000` on the H616.

| Offset | Register | What the sources say |
| --- | --- | --- |
| `0x000` | mode | bits 3:0 select the decoder (0 MPEG, 1 H.264, 4 H.265, 7 none); bit 6 enables the input stage and bit 7 the encoder (Bootlin, and the vendor kernel's interrupt handler for every part but the oldest); bit 5 is the JPEG decoder at `0xe00` on the sun50i parts (vendor kernel); bits 17:16 the memory interface width (3 for 128 bits), bit 20 the reconstruction write mode, bits 21 and 22 for pictures wider than 2048 |
| `0x004` | reset | bit 24 resets the encoder, bit 16 the decoder, bit 0 everything; bits 8 and 9 read as "idle" (Bootlin) |
| `0x0f0` | version | the engine's version in the upper 16 bits |

Selecting the encoder, two forms:

- **older** (jemk): the mode register is written `0x0013000b`: engine 11 in
  the low four bits. The vendor kernel reads the select this way only on its
  oldest part.
- **newer** (Bootlin for the V3s): bits 7 and 6 set and the decoder field 7,
  after a pulse of the encoder's reset bit. The vendor kernel's handler for
  the sun50i parts tests exactly these bits, so this is the form expected on
  the H616.

## The input stage (`0xa00`)

| Offset | Older map (jemk) | V3s map (Bootlin) |
| --- | --- | --- |
| `0xa00` | input size: width in macroblocks at 31:16, height at 15:0 | picture info: the same |
| `0xa04` | input stride: macroblocks at 31:16 | stride: input at 26:16, thumbnail at 9:0 |
| `0xa08` | control: the colour format from bit 29 (0 NV12, 1 NV16) | control: format from bit 27 (0 NV12, 4 NV21, 24 and 26 planar), mirror bit 22, rotation 21:20, thumbnail bit 19, scaler bit 16, colour space 2:1, interrupt enable bit 0 |
| `0xa10` | | status (bit 0; from the vendor kernel) |
| `0xa2c` | | scaler size: height in macroblocks at 26:16, width at 10:0 |
| `0xa70`, `0xa74` | | output luma and chroma addresses |
| `0xa78` | input luma address | the same |
| `0xa7c` | input chroma address | the same (first chroma plane) |
| `0xa80` | | second chroma plane |

The two maps agree where both speak, except in the format field: what the
older map calls NV16 (1 from bit 29) is the value the newer one calls NV21
(4 from bit 27).

## The encoder (`0xb00`)

| Offset | Older map (jemk) | V3s map (Bootlin) |
| --- | --- | --- |
| `0xb04` | parameters: bit 4 P slice, bit 8 CABAC, bit 31 no emulation prevention | `PARA0`: picture type 1:0 and reference type 3:2 (frame, top, bottom), slice type 5:4 (0 I, 1 P, 2 skipped P, 3 B), entropy bit 8, CABAC init method bit 9, fix mode number 11:10 (carries `cabac_init_idc`), deblocking mode 13:12, IDR picture id bit 15, alpha offset 19:16, beta offset 23:20, frame number 27:24, reference reorder bit 28, slice mode bit 29, zero stuffing bit 30, no emulation prevention bit 31 |
| `0xb08` | QP: written `(4 << 16) \| (qp << 8) \| qp` | `PARA1`: fixed QP 5:0, rate-control mode from bit 6 (0 fixed), dynamic motion estimation bit 8, tiled output bit 9, input stride in macroblocks divided by 48 at 13:10, chroma QP offset 18:16, temporal filter bit 19, motion vector clipping 25:24 and 26, mode optimisation 30:28 and 31 |
| `0xb0c` | | `PARA2`, written 0 |
| `0xb10` | motion estimation: written `0x00000104` | `ME_PARA`: intra prediction off bit 0, fine search level 2:1, search switches bits 3 to 5, coarse cost 7:6, motion estimation off bit 8, skipped macroblocks off bit 9, bits 11 to 15, no motion vector write-back bit 16, region of interest bit 17, deblocking and third-pixel data to memory bits 18 and 19, quarter-pixel switches bits 20 and 21. Written `0x00010004` |
| `0xb14` | control: interrupt enables, written `\|= 0xf` | `INT_EN`: finish bit 0, stall bit 1, macroblock overtime bit 2 |
| `0xb18` | trigger: `0x1 \| bits << 8` puts bits, `0x8` starts an encode | `STARTTRIG`: type 3:0 (1 put bits, 2 byte align, 4 and 5 signed and unsigned Exp-Golomb, 8 start, 9 continue), number of bits 13:8, mode 17:16 (0 H.264, 1 JPEG, 2 VP8) |
| `0xb1c` | status: written back to clear; `(status & 3) == 1` is success | `STATUS`: finish bit 0, stall bit 1, macroblock overtime bit 2, overtime bit 3, bit writer ready bit 9 |
| `0xb20` | the bits to put | `PUTBITSDATA` |
| `0xb24` | | macroblocks over time |
| `0xb28` | | cyclic intra refresh: enable bit 31, start column 23:16, end column 7:0 |
| `0xb2c` to `0xb40` | | rate control: init, four MAD thresholds, MAD sum |
| `0xb44` | | temporal filter |
| `0xb48`, `0xb4c` | | dynamic motion estimation thresholds |
| `0xb50`, `0xb54`, `0xb58`, `0xb5c` | | statistics: MAD, residual bits, header bits, motion estimation info |
| `0xb60` | a buffer "of unknown purpose", one per stream | motion vector buffer, written 0 |
| `0xb80` | bitstream buffer address | the same |
| `0xb84` | bitstream buffer end (address + size - 1) | the same |
| `0xb88` | bit offset, written 0 | the same |
| `0xb8c` | size in bits | the same |
| `0xb90` | bits written so far | the same |
| `0xba0`, `0xba4` | reference luma and chroma | reference 0 luma and chroma |
| `0xba8`, `0xbac` | | reference 1 luma and chroma |
| `0xbb0`, `0xbb4` | reconstruction luma and chroma | the same |
| `0xbb8` | the reference's "smaller luma" | sub-pixel buffer of the last picture |
| `0xbbc` | the reconstruction's "smaller luma" | sub-pixel buffer of this picture |
| `0xbc0` | a line buffer "of unknown purpose" | macroblock info buffer |
| `0xbc4` | | deblocking buffer, written 0 |
| `0xbc8` | | temporal filter counts |
| `0xbcc` to `0xbdc` | | region of interest: QP offset and four areas |

The block's layout is one and the same in both maps. They differ in what
they write into it:

| | Older map | V3s map |
| --- | --- | --- |
| `0xb08` bits 13:8 | a second QP | the input stride divided by 48 (the newer engine needs it: a 1920 wide picture gives 3) |
| `0xb08` bits 18:16 | 4 | the chroma QP offset (4 by default: the same value) |
| `0xb10` | `0x104`: bit 2, bit 8 | `0x10004`: bit 2, bit 16. Bit 8 is "motion estimation off" in the newer map |
| `0xb60` | the address of a buffer | 0, with the write-back switched off by bit 16 of `0xb10` |
| Interrupt enables | `0xf` | `0x3` |
| Before a put of bits | nothing | wait for bit 9 of the status |
| Encoder reset | none | a pulse of bit 24 at `0x004` |
| After the headers | nothing | wait for the idle bits 8 and 9 at `0x004` |

## The order of an encode

**Older map**, every frame:

1. Select engine 11.
2. Bitstream: `0xb88` = 0, `0xb80` = address, `0xb84` = address + size - 1,
   `0xb8c` = size in bits.
3. Headers through the bit writer: the data to `0xb20`, then `0xb18` =
   `1 | bits << 8`, at most 32 bits at a time. A start code is put with
   emulation prevention off (bit 31 of `0xb04`): 24 zero bits, then
   `0x100 | NAL header` in 16. SPS and PPS are put before the first frame,
   a slice header before every frame; the trailing bits of a parameter set
   are computed from `0xb90`.
4. Input: `0xa04`, `0xa00`, `0xa08`, `0xa78`, `0xa7c`.
5. Reconstruction: `0xbb0`, `0xbb4`, `0xbbc`. For a P frame the reference:
   `0xba0`, `0xba4`, `0xbb8`. Two sets of buffers take turns.
6. `0xbc0` and `0xb60`: the two buffers of unknown purpose.
7. `0xb14 |= 0xf`, `0xb1c |= 0x7`.
8. `0xb04` = `0x100` for CABAC, `0x10` for a P frame; `0xb08`; `0xb10`.
9. `0xb18` = 8; wait for the interrupt.
10. Read `0xb1c` and write it back; the length is `0xb90 / 8` bytes.

**V3s map**. Once per stream: the mode register's encoder bits cleared, the
encoder reset pulsed, the encoder bits set; then the input stage: `0xa00`,
`0xa2c` (the same size), `0xa04`, `0xa08` = 0 (NV12, no rotation, BT.601),
`0xa78`, `0xa7c`. Every frame:

1. `0xb18` = 0.
2. Bitstream: `0xb88` = 0, `0xb80`, `0xb84`, `0xb8c`, and `0xb90`, `0xb58`,
   `0xb54` = 0.
3. Headers: emulation prevention off; for every put wait for bit 9 of
   `0xb1c`, then `0xb20` and `0xb18` = `1 | bits << 8`; a start code is four
   bytes put one by one; a parameter set ends with its stop bit and zero
   bits up to a byte, counted from `0xb90`; emulation prevention on again;
   wait for the idle bits at `0x004`. SPS and PPS go before an IDR frame.
4. `0xbc0` = macroblock info; `0xb60` = 0; `0xbb0`, `0xbb4` =
   reconstruction; `0xba0`, `0xba4` = the previous reconstruction (for an I
   frame the frame's own); `0xbbc` = this frame's sub-pixel buffer, `0xbb8`
   = the last one's; `0xbc4` = 0; `0xb28` = 0.
5. `0xb04`: slice type, entropy mode, `cabac_init_idc`, deblocking offsets;
   the frame number field stays 0. `0xb08`: QP, stride divided by 48,
   chroma QP offset. `0xb0c`, `0xb48`, `0xb4c`, `0xb2c` to `0xb3c` = 0.
   `0xb10` = `0x10004`. `0xb50`, `0xb24`, `0xb5c` = 0.
6. `0xb14` = 3; `0xb18` = 8.
7. At the interrupt: `0xb1c & 0xf`, bit 0 is success; `0xb1c` = `0xf` to
   clear; `0xb14` = 0. The length is `0xb90 / 8` bytes.

The vendor kernel's handler, which the lab module copies: with bit 7 of the
mode register set, read `0xb1c & 0xf` and `0xb14 & 0x7`; if both are not
zero, clear the three enable bits and wake whoever waits.

## Buffers

For a picture of W by H macroblocks; in bytes for 1920x1088 (120 by 68).

| Buffer | Older map | V3s map |
| --- | --- | --- |
| Input | NV12: W16 x H16 x 3/2, luma and chroma in one buffer (3133440) | the same |
| Bitstream | 1 MiB | the capture buffer of the request |
| Reconstruction, each of two | luma `ALIGN(W16, 32) x ALIGN(H16, 32)` (2088960) and chroma `ALIGN(W16, 32) x ALIGN(H8, 32)` (1044480) in one buffer | luma `ALIGN(W, 2) x 16 x ALIGN(H + 1, 4) x 16` (2211840) and chroma `ALIGN(W, 2) x 16 x ALIGN(ceil(H / 2), 4) x 16` (1105920) in one buffer, rounded up to 4 KiB |
| Sub-pixel, each of two | a quarter of the luma size (522240) | `(ALIGN_DOWN((W + 47) x 2 / 3, 32) + ALIGN(W, 32) x 2) x ((H x 16 + 72) / 8)` (352 x 145 = 51040) |
| `0xbc0` | `W x 32` (3840) | `ceil(W / 32) x 4096` (16384) |
| `0xb60` | `ALIGN(W, 4) x H x 8` (65280) | none |

The lab gives every buffer the larger of the two sizes and room beyond it,
with markers at both ends of the part in use.

## The stream's headers

Both sources write the same syntax, which is also what the product's x264
makes today: one SPS and one PPS with id 0, one reference frame, frames
only, one slice per picture, deblocking controlled per slice. They differ in
two choices, both free:

- picture order: type 2 (jemk: output order is decoding order, nothing in
  the slice header) or type 0 with an 8-bit counter (Bootlin). Type 2 is
  what a stream without B frames needs and what x264 writes for it.
- `frame_num` width: 4 bits (jemk) or 8 (Bootlin).

The lab tool writes its own headers: constrained baseline (profile 66 with
`constraint_set0` and `constraint_set1`), the level for the size and rate,
picture order type 2, cropping for sizes that are not whole macroblocks, an
IDR frame with SPS and PPS before it whenever a key frame is asked for.

## Lab record

Nothing yet.

## The H616's register description

Nothing yet: written from V1 on.
