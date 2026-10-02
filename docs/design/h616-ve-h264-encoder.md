# The H616 video engine's H.264 encoder: sources, register maps, lab method

Plan: `docs/baselines/work-plan-20261002-hardware-h264.md`. This note holds
what is known about the encoder from open sources, the two register maps
that are the hypotheses to test, how the lab works, and, as the work goes
on, the lab's record and the register description that comes out of it.

Status (2026-10-02, V2.1 and V2R): the engine answers, its encoder block is
the newer map's and its bit writer works, but it wrote its bits into the
kernel's memory instead of the buffer it was given. Work on the board has
stopped until that is explained; nothing has been encoded. A candidate
explanation is recorded below (the stream address counts in units of 256
bytes), and the instrument that can confirm it without the engine writing
anything, a trace of the vendor's encoder library against a stand-in, is
built and proven on the development host.

## Sources

| Source | Licence | Version | What is taken |
| --- | --- | --- | --- |
| `jemk/cedrus`, `h264enc/` | GPL-2.0-or-later | commit `84c5e37` (2015-01-18) | the register sequence of an encode on the A10 to H3 generations, the buffers and their sizes, headers written through the engine's bit writer |
| `jemk/cedrus`, `common/ve.h`, `common/ve.c` | LGPL-2.1-or-later | the same commit | register names and offsets, how an engine is selected |
| `bootlin/linux`, branch `cedrus/h264-encoding`: `cedrus_regs.h`, `cedrus_enc.c`, `cedrus_enc_h264.c` | GPL-2.0 | commit `4e94979` (2023-11-10) | the map of the V3, V3s and S3 with its fields, the encode sequence, the buffers and their sizes, the encoder's reset, the interrupt handling |
| Linux 5.19, `drivers/staging/media/sunxi/cedrus` | GPL-2.0 | v5.19 | what a driver of this engine does at probe: interrupt, SRAM section, clocks, reset, runtime power |
| The vendor kernel's `drivers/media/cedar-ve/cedar_ve.c` (`orangepi-xunlong/linux-orangepi`, branch `orange-pi-4.9-sun50iw9`) | GPL-2.0-or-later | read on 2026-10-02 | which bits of the top register mean encoder, input stage and JPEG decoder on the sun50i parts, where their interrupt status and enables are, and how the handler quiets them |
| Linux 5.19.4, `drivers/staging/media/sunxi/cedrus/cedrus_regs.h` | GPL-2.0 | v5.19.4 | the forms in which this engine's decoder blocks take an address: as it is (the H.264 decoder's pictures), with its top four bits moved to the bottom (the H.264 decoder's stream), and in units of 256 bytes (the H.265 decoder's stream, its end, and its picture and side buffers) |
| `Allwinner-Homlet/H6-CedarC`, `ve/veAw/veAw.c` and `ve/include/veAwTopRegDef.h` (the vendor's own publication of its user-space engine keeper for the H6; its decoder half only) | LGPL-2.1 (the files' notice) | commit `d92ffcb` (2018-03-29) | how the engine's version is put together where `0x0f0` reads zero: `0x0e0` (the decoder's) in the upper half of a 64-bit number and `0x0e4` (the encoder's) in the lower, `0x3101000012010` on the H6; that the H6's keeper takes nothing off a buffer's address before the engine gets it and tells chip id 0; the fields of the top level's mode, reset and status registers; how the memory type becomes the mode register's memory field |
| The vendor's "Video Encoder API Guide" (R-E-17, V1.0, 2016), as shipped beside the library | documentation | | the order in which the encoder library is called and what its parameters mean |
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
2. Watched, only if the operator approves it (plan, V2R; approved on
   2026-10-02 in the form without an engine, see the lab record): the
   vendor's encoder library as a black box, called through its published
   interface. What is recorded is its traffic with what it takes for the
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
| `0x000` | mode | bits 3:0 select the decoder (0 MPEG, 1 H.264, 4 H.265, 7 none); bit 6 enables the input stage and bit 7 the encoder (Bootlin, and the vendor kernel's interrupt handler for every part but the oldest); bit 5 is the JPEG decoder at `0xe00` on the sun50i parts (vendor kernel); bits 17:16 the memory interface width (3 for 128 bits), bit 20 the reconstruction write mode, bits 21 and 22 for pictures wider than 2048. The vendor's keeper for the H6 names the rest: bits 8 and 9 choose what two counters count, 10 and 11 gate the decoder's and the encoder's clock, 13 to 15 switch parts of the decoder off, 18 a memory self-test, 19 a macroblock counter, 4, 12, 26, 27 and 29 the compressed frame buffer format, 31 a RAM power-down. Its memory field: 0 for 16-bit DDR1, 1 for 32-bit DDR1 or 16-bit DDR2, 2 for 32-bit DDR2 or 16-bit DDR3, 3 for wider DDR3 with bit 20 set |
| `0x004` | reset | bit 24 resets the encoder, bit 16 the decoder, bit 0 everything; bits 8 and 9 read as "idle" (Bootlin). The vendor's keeper for the H6: bit 8 "writes to memory finished", bit 9 the decoder's "sync idle", bit 10 the encoder's, bit 11 the JPEG decoder's; bit 17 masks the decoder's requests and bit 25 the encoder's; bit 18 resets a part of the decoder; bit 4 masks, and bit 5 clears, the memory synchroniser |
| `0x01c` | status | bit 16: the memory synchroniser is idle (the vendor's keeper for the H6); the low bits are the older generations' interrupt flags, bit 8 a timeout enable |
| `0x0e0`, `0x0e4` | identification | the decoder's and the encoder's, where `0x0f0` reads zero (the vendor's keeper for the H6) |
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

## The lab

### Parts

- **The lab module** `hardened-blikvm-ve-lab` (kernel, GPL-2.0-only, root
  only, never packaged). It binds to the engine's node while `cedrus` is
  unbound for the session, takes the interrupt, the SRAM section, the three
  clocks and the reset in the order `cedrus` does, and offers one client:
  the register window mapped, DMA buffers allocated and mapped with their
  bus addresses told, a wait for the encoder's interrupt, a reset of the
  engine. Closing it, or the client's death, resets the engine and frees
  everything.
- **The lab tool** `bli-kvm-ve-lab` (user space, static aarch64): the
  picture generator, the header writer, the register plans of both maps,
  the experiments, PSNR, and a software stand-in for the engine that makes
  all of it testable on the host.
- **The session script** on the development host: copies both to the
  board's tmpfs, checks the lease rule, starts the watchdog keeper, runs
  the experiment, collects the receipt and the kernel log, and puts the
  board back as it was.

### Rules for experiments

1. The engine is programmed only while nobody holds the HID lease; otherwise
   the run waits (the operator's answer of 2026-10-02). The lease cannot be
   seen from outside its holder: neither the owner nor the server tells a
   third party whether one is live. Nobody can hold one, though, without a
   page that renews it at least every 30 s, over HTTPS or over a QUIC
   session. So a session starts only when the board has no established
   HTTPS connection and no open QUIC session, checked three times over
   eight seconds; this also waits for somebody who only watches.
2. Before any trigger every address register the maps name holds an address
   inside the session's own buffers, and the bitstream's end register bounds
   what the engine may write. An address of zero is not memory on this SoC:
   its memory begins at `0x40000000`.
3. First runs use a small picture in buffers many times its size, with
   marker bytes around the part in use; the markers are checked after every
   run.
4. A keeper process arms the SoC's watchdog (16 s) for the length of a run:
   a kernel that hangs is reset instead of waiting for a hand. `sync`
   precedes every run. The engine is reset before the watchdog is disarmed.
5. The mask probe writes no trigger. Registers outside the blocks the open
   maps name are read, never written.
6. Afterwards: the lab module unloaded, `cedrus` bound and suspended as
   found, the watchdog disarmed, no file left on the board, no unit or
   process. The kernel log is read after every session; a warning it did
   not have before ends the session.
7. A hang or reboot that is not understood stops the work on the board
   until it is explained in the record below; a second one ends the plan.
8. (Since V2.1.) Rule 2 assumed that the engine writes where its address
   registers say. It does not always. No experiment may make the engine
   write to memory until it is known, by means that write nothing, where
   that write goes.
9. (Since V2.1.) After any session in which the engine could have written,
   `tools/device/ve_memory_scan.py` reads all of memory for what it was
   given. A find outside the session's buffers means the kernel's memory is
   damaged: nothing else is done on the board until it has been reset.

### Test pictures

A generator in the lab tool, the same on the board and on the host, makes
every sequence from its name, size and frame number alone; no screen of the
operator is recorded.

| Name | What moves |
| --- | --- |
| `scroll` | a page of text-like marks that scrolls by lines |
| `drag` | a window dragged over a still desktop |
| `motion` | the whole picture: moving gradients and blocks, the hardest case |
| `pointer` | a still desktop with a small pointer that moves |

Sizes: 1920x1080, 1280x720, 1366x768, and 320x240 for first runs. Pictures
are NV12, BT.601 limited range, as the product's converter makes them.

### Measures

- **Decodes**: the board's ffmpeg decodes the stream without an error and to
  the expected number of frames; on the host Chromium decodes it with
  WebCodecs.
- **PSNR**: of the luma plane, per frame, between the generator's picture
  and the decoded one; a sequence's value is the mean over its frames.
- **Time per frame**: from the write of the start trigger to the finish
  status, by the board's monotonic clock.
- **Rate**: the bytes of the frames in every window of ten seconds at the
  sequence's frame rate.
- **The reference**: x264 with the product's settings (`ultrafast`,
  `zerolatency`, constrained baseline, the same bitrates), driven through
  the encoder crate's own wrapper on the same pictures on the board.

### Receipts and the log

Every experiment writes one JSON receipt under `evidence/private`
(`ve-lab-<phase>-<date>-<name>.json`): the tool's version and the source
commit, the engine's version, the map and parameters, every register written
with its value in order, the status and length read back, the buffers'
sizes and marker checks, the times, the decoders' verdicts and PSNR, and
the kernel log lines of the session. Bus addresses are recorded as offsets
within their buffers.

`evidence/private/ve-lab-log.md` is the lab's diary: when a session began
and ended, what it tried, what came out, and the hours spent in each phase,
which the plan's time boxes count.

## A module for this kernel

`scripts/build-ve-module.sh` builds modules for the board's kernel, which
its vendor built without publishing a tree or a headers package. The tree is
made of the upstream source of the same release (5.19.4, signature checked),
the board's configuration (`/proc/config.gz`), the compiler the kernel names
in `/proc/version` (Arm's GNU toolchain 8.3-2019.03), and the kernel's own
headers from its `kheaders` module, laid over the upstream ones.

What `prepare` found, and checks on every run:

- After `olddefconfig` with that compiler every option of the rebuilt
  configuration header has the board's value, and the release string is the
  board's (`5.19.4-sunxi64`).
- The board's configuration header has 42 options more. They belong to
  patches the vendor's kernel carries and upstream lacks (Wi-Fi drivers,
  AUFS, the AC200, sound, panels), and one upstream option that a patch
  made selectable (`GPIO_SYSFS`).
- One of those patches changes a structure every driver with a mapping
  uses: with AUFS, `struct vm_area_struct` has one member more. A module
  built with the rebuilt configuration header would lay that structure out
  wrongly. The build therefore uses the board's headers and the board's
  configuration header; the rebuilt one only proves that source,
  configuration and compiler belong together.

This changes one sentence of the plan, which asked that the rebuilt
configuration header define exactly what the board's defines. It cannot:
upstream does not know the vendor's options. The check is that it defines
nothing differently.

Modules are built from a copy under a fixed name, with the paths of the
build mapped away; two builds from different directories gave the same
bytes. Their version magic must equal the board's, and a module that names
a path of the build machine is refused. The module build is not part of
`check.sh`: it needs the board's headers, which are not in the repository.

The check modules (`kernel/hardened-blikvm-ve/check`): `ve_hello` does
nothing; `ve_probe_test` registers a platform driver that matches nothing
and a device node with one request and one mapping. `struct module` has the
size it has in the board's own modules (0x300).

## Lab record

### V1.1, 2026-10-02: modules built by the recipe load

`tools/device/ve_lab_session.sh` was first run with a harmless command of
25 s: the quiet check passed, the watchdog was armed across its 16 s
timeout and disarmed, the board stayed up, nothing was left.

Then the two check modules (`ve_hello.ko` `5acb75aa...`, `ve_probe_test.ko`
`36b04534...`) in a session: `ve_hello` loaded and unloaded; the test
device answered its request, refused another, and its page was read and
written through the mapping; both unloaded. The kernel's lines of the
session were the two taint notices (an out-of-tree module, and an unsigned
one) and the modules' own. The taint flags went from 1024 (the staging
driver `cedrus`) to 13312 and stay so until the next boot. `cedrus` was
bound and suspended afterwards.

Receipts: `evidence/private/ve-lab-v1-20261002-session-dry`,
`ve-lab-v1-20261002-module-check`.

### V1.3, 2026-10-02: the engine answers

The lab module (`kernel/hardened-blikvm-ve`, `ve_lab.c`) bound to the
engine's node after `cedrus` was unbound; the lab tool opened its device,
which gives the engine its clocks and releases its reset, and read the four
blocks. Nothing was written. Afterwards `cedrus` was bound and suspended
again, and the kernel had said two lines: the lab module bound, the decoder
registered.

| Register | Value after power-on |
| --- | --- |
| `0x000` mode | `0xc0000007`: no decoder; bits 31 and 30 set |
| `0x004` reset | `0x00000300`: the two idle bits |
| `0x01c` | `0x00010000` |
| `0x030` | `0x00000200` |
| `0x040` | `0x0000000f` |
| `0x080`, `0x084`, `0x088` | `0x00001c55`, `0x000fffff`, `0x00008000` |
| `0x0e0`, `0x0e4` | `0x00033010`, `0x00012011` |
| `0x0f0` version | `0x00000000` |

- **The version register reads zero.** The older generations have their
  version in its upper half; this engine does not. `0x0e0` and `0x0e4` hold
  values that look like an identification, and no open source names them.
  They are what this note records as the engine's version.
- The input stage (`0xa00`), the encoder (`0xb00`) and the JPEG decoder
  (`0xe00`) read zero in every register while none of them is selected.

Receipt: `evidence/private/ve-lab-v1-20261002-probe`.

### V1.4, 2026-10-02: buffers

Six buffers from the lab module, from 4 KiB to 16 MiB: each arrived zeroed,
kept a pattern written through its mapping, and has a bus address inside
the board's memory (the single page at `0x46df5000`, the others between
`0x77000000` and `0x78c00000`, where the kernel's CMA area lies). The CMA
total was the 131072 kB found before; closing the device freed them.

Receipt: `evidence/private/ve-lab-v1-20261002-buffers`.

### V1.5, 2026-10-02: which bits the encoder's registers hold

With the encoder selected and nothing triggered, every register the open
maps name (34, no trigger, status or interrupt register among them) was
read, written with ones, read, written with zero, read, and put back.

**The older select does nothing here.** With `0x0013000b` in the mode
register, engine 11 in the low four bits, no register of the input stage or
of the encoder holds a bit.

**The newer select is this engine's.** After the pulse of the encoder's
reset bit, bits 7 and 6 stay set in the mode register (`0xc00000c7`), and
the blocks hold what is written:

| Register | Bits that can be set | Remark |
| --- | --- | --- |
| `0xa00` size | `0x07ff07ff` | 11 bits for each dimension in macroblocks |
| `0xa04` stride | `0x07ff07ff` | the same two fields |
| `0xa08` control | `0xf8790006` tried, all held | only the bits the map names were tried, never the interrupt enable |
| `0xa2c` scaler size | all | |
| `0xa78`, `0xa7c` input | `0xfffffff0`, `0xfffffff8` | luma addresses in steps of 16 bytes, chroma of 8 |
| `0xb04` `PARA0` | all | |
| `0xb08` `PARA1` | all | `0x0000001e` after reset: QP 30 |
| `0xb0c` `PARA2` | all | |
| `0xb10` motion estimation | `0x00ffffff` | |
| `0xb24` | `0x0003f000` | |
| `0xb28` intra refresh | `0x80ff00ff` | the map's three fields exactly |
| `0xb2c` | `0xf03fffff` | |
| `0xb30`, `0xb34`, `0xb38` | all | |
| `0xb3c` | none | the map's fourth MAD threshold holds nothing |
| `0xb44` temporal filter | `0xffff7fff` | the map's five fields exactly |
| `0xb48`, `0xb4c` dynamic motion estimation | `0x03ff03ff` | the map's two fields exactly |
| `0xb60` | `0xfffffffc` | an address |
| `0xb80` stream start | `0xfffffffc` | |
| `0xb84` stream end | all; the low two bits cannot be cleared | all ones after reset |
| `0xb88` bit offset | none | |
| `0xb8c` size in bits | `0xffff0000` | in steps of 65536 bits, 8 KiB; `0xffff0000` after reset |
| `0xba0`, `0xba4`, `0xbb0`, `0xbb4`, `0xbb8`, `0xbbc`, `0xbc0`, `0xbc4`, `0xbc8` | `0xfffffffc` | addresses |

Every register read after the probe what it had read before it. No
interrupt came. The kernel's lines were the two of every session.

What this says:

- The block is the V3s map's: where that map names fields, the bits that
  can be set are those fields and no others (intra refresh, temporal
  filter, dynamic motion estimation, the two size fields).
- Three things the maps do not say: the stream buffer's size counts in
  steps of 8 KiB; its end address always has its low two bits set; the bit
  offset cannot be read back.
- One register of the map is not there: the fourth MAD threshold.

Receipt: `evidence/private/ve-lab-v1-20261002-masks`.

### Gate V1: passed

Modules built by the recipe load. The engine's identification is recorded
(its version register reads zero). The encoder's block holds what is
written to it once the encoder is selected the newer way. Buffers work.
After every session `cedrus` was bound and suspended as before, and the
kernel's log had only the lab module's and the decoder's own lines. The
first hypothesis for V2 is the V3s map; the older map is tried only for
what it writes into the block, not for its select.

### V2.1, 2026-10-02: the bit writer works, and writes to the wrong place

Three sessions put SPS and PPS of a 320x240 stream through the engine's bit
writer, once and then 8, 64 and 700 times over, with the encoder selected
the newer way and the stream buffer described as both maps say: bit offset
0 at `0xb88`, start at `0xb80` (`0x77410000`, 64 KiB into a lab buffer),
end at `0xb84` (4 MiB above), size at `0xb8c` (`0x02000000` bits). Nothing
was encoded.

What the engine did:

- **The bit writer is the maps'.** The status register `0xb1c` reads
  `0x00000260` at rest: bit 9, "ready for bits", and two bits no map names.
  Every put was taken, and `0xb90` counted exactly the bits that were put:
  200 for one SPS and PPS, 140000 for 700 of them.
- **Nothing arrived in the buffer.** Its first bytes stayed zero at every
  length, and every guard kept its markers.
- **The bytes went to physical `0x41000000`.** The next session's own
  `sync` died in the kernel, before that session touched the engine,
  calling an address made of bytes of the PPS. A scan of all memory
  (`tools/device/ve_memory_scan.py`, reading only) then found the
  parameter sets 16 MiB above the beginning of memory, 696 of the 700
  repeats in a row: in the kernel's read-only data, which the kernel maps
  from `0x40fa0000` to `0x413bffff` on this board. The engine writes its
  bits to memory in portions (the last 100 bytes were not there, so a
  portion is between 128 bytes and 1 KiB), but not at the address in
  `0xb80`.
- One more session gave the engine the same buffer's address counted from
  the beginning of memory (`0x37410000`), as the older generations count.
  The buffer stayed empty again. Whether that run also wrote to
  `0x41000000` cannot be told: it would have written the same bytes.

Why `0x41000000` is not known. It follows from neither address by dropping
or adding bits. Sixteen MiB is four times the buffer's size and half its
size in bits; that may be chance.

What it did to the board. The engine overwrote 17 KiB of the kernel's
tables. Two processes died in the kernel on pointers that hold bytes of the
PPS (`sync` in `sync_fs_one_sb`, and `kswapd0` in `super_cache_count`).
PID 1, the journal and the login service then hung in the kernel; the web
interface went on serving. No write was waiting for the card (`Dirty` and
`Writeback` were zero). The damage is in memory, and a reset removes it.
The lab was not permitted to reset the board; the operator was told to.

What follows:

- Gate V2 is not reached, and by rule 7 the work on the board has stopped.
- Rules 8 and 9 above. Markers around the buffers and the bounds registers
  could not have caught this: the engine never came near the buffers.
- Ways to the explanation that make the engine write nothing: reading, not
  writing, the rest of the engine's register window and the state of the
  SoC's IOMMU and memory bus after a reset; and watching the vendor's
  library (V2R) without an engine at all, against a register window that is
  plain memory, which shows how it describes the stream buffer to this
  engine.

Receipts: `evidence/private/ve-lab-v2-20261002-bits-a`, `-bits-b`,
`-bits-c`, `-incident`.

### V2.1 afterwards, 2026-10-02: a candidate explanation

The record above says that `0x41000000` follows from neither address by
dropping or adding bits. It does follow from both by a shift:

    0x77410000 << 8 = 0x77_41000000, and its low 32 bits are 0x41000000
    0x37410000 << 8 = 0x37_41000000, the same

That is what an engine does that takes the register for the buffer's
address in units of 256 bytes. This engine has such registers: Linux'
`cedrus` writes every address of the H.265 decoder block that way (the
stream, its end, the picture buffers), while the older H.264 decoder block
beside it takes its addresses in two other forms. The H.264 encoder of the
V3s takes its stream address as it is (Bootlin's driver); the H616's is a
later revision of the same block (encoder identification `0x00012011`), in
a part whose memory can lie above what 32 bits address.

What fits:

- Both sessions give one place, as the scan found only one.
- The registers' bits (V1.5): the start register holds every bit but the
  low two, the end register has its low two bits always set. In units of
  256 bytes that is a start on a 1 KiB boundary and an end on the last byte
  before one.
- The end register read all ones after reset and was written `0x7780ffff`;
  shifted, that is an end far above the start, so the bit writer never
  thought its buffer full.

What it does not show: the same form for the other address registers of the
block (reference, reconstruction, sub-pixel, macroblock info), whose low
two bits are also fixed, or for the input stage's, whose low four and three
are. Each may have its own form, as in the decoder blocks.

It stays a candidate until something that makes the engine write nothing
confirms it (rule 8): the vendor's library, which must describe a stream
buffer to this block, watched without an engine (below). If it holds, the
first experiment afterwards can be made safe under both readings: a stream
register written with a lab buffer's address divided by 256 names that
buffer in units of 256 bytes, and as a plain address it names a place
between `0x00400000` and `0x00800000`, far below this part's memory, which
begins at `0x40000000`. That nothing else lives there is to be read from
the board's own resource map before such a run.

### V2R, 2026-10-02: the vendor's encoder without an engine

**V2R.1, the operator's answers.** After the incident the operator was
asked how to go on, and answered: read only on the board, and watch the
vendor's library without the engine. Asked later where that library may
run, the operator answered: on the test board only, not under an emulator
on the development host; and that the vendor's own LGPL publication of its
engine keeper for the H6 may be kept in the cache and read. That changes
four sentences of the plan, for these reasons:

| The plan | Now | Why |
| --- | --- | --- |
| V2R follows a missed gate V2 | the trace of V2R is used inside V2, as the way to the explanation that rule 8 asks for | no experiment that could reach gate V2 may run before the stray write is explained, and the library is the one source that must know how this block takes a stream address |
| V2R.2: the library gets the engine through the lab module | the library gets no engine: a stand-in behind the register window | an engine that writes where it should not must not be driven by code whose writes are not yet known either |
| V2R.3: the trace is proven first on the board | proven first on the development host, then on the board at the beginning of every session | the trace's method needs no engine; the host proves everything but the breakpoint, and the board proves that |
| V2R.4: the rules for experiments apply to the vendor's runs | they apply to the lab's own experiments that follow | a run without an engine touches no hardware |

The library: `libvencoder.so` for aarch64 and glibc from a public mirror of
the vendor's CedarC package (`aodzip/libcedarc`, commit `e4246be`,
`library/toolchain-sunxi-aarch64-glibc`; SHA-256 `68b7908b...9aa74`), with
the interface headers of the same tree. It was built for another part of
the vendor's range, and one library serves many: it asks its keeper which
engine it drives. A second mirror (`CalvinXu17/libcedarc`, commit
`e68d4a7`) carries a later build split into parts whose callable front
exists only as source under a proprietary notice; that source is not read,
and that build is not used.

What is done with it: it is kept in a cache outside the repository, copied
to the board's tmpfs for the length of a session, loaded there by the trace
program as an unprivileged user, called, and unloaded; the session's
directory is removed afterwards. It is not disassembled,
decompiled or read, and no file of it enters Git, a package, a slot or an
image (`tools/test_packaging_boundaries.py` now also refuses the names of
the trace and of the library in everything that is built for a board, and
any vendor header or binary library among the tracked files).

**V2R.2, how the library gets what it takes for an engine.** Its interface
lets the caller supply both things it needs from outside: the engine's
keeper (lock, reset, the wait for the interrupt, where the registers are,
which engine this is, the offset taken off addresses) and the allocator
(buffers, and the address of each for the engine). So nothing of the
vendor's kernel is involved and no shim for its device: `tools/ve-trace`
fills both tables (`vendor_ops.c`), records every call, and answers:

- the registers are the closed window;
- the engine's version is what the vendor's own keeper would compute from
  this engine's registers: `0x0003301000012011`;
- buffers come from an arena of this process, each with a made-up address
  in a range chosen for the run. Nothing lives at those addresses. They
  only have to be found again in what the library writes, and since the
  range can be moved from run to run, the form in which an address is
  written shows itself;
- the interrupt "comes" when the library waits for it after it has started
  an encode: the stand-in sets the finish status and a length.

The library imports nothing that reaches outside its process but `fopen`
and `printf`; `fopen` is answered by the trace program, recorded and
refused. Its three companion libraries are replaced by empty ones of the
same names.

**V2R.3, the trace.** As the plan describes it, with the window being
memory instead of the engine: the window is closed to the code under the
trace; an access faults; the handler opens the window for reading and
arranges for a stop behind that one instruction; if the instruction faults
again it is a store, and the window is opened for writing too; the
instruction runs against the window's memory; at the stop the window is
closed again and the access is recorded: for a load the word at the
faulting address, for a store every word that changed (or the word at the
faulting address if none did). Then the stand-in is told, and what it
answers is what the register reads from then on. No instruction is decoded
or emulated. The stop is a breakpoint in the place of the next instruction
on AArch64 (the handler keeps the word it replaces and puts it back without
looking at it) and the processor's single-step flag on the development
host.

A session on the board (`tools/device/ve_trace_session.sh`) is carried by
the lab's session script with its rules (a quiet board, nothing left
behind), loads no module and does not touch the engine: `cedrus` stays
bound and suspended throughout. Everything runs as the unprivileged user
`nobody` with no new privileges.

The stand-in (`model.c`) is the engine as V1.3 and V1.5 measured it: the
reset values, the encoder's blocks reading zero until the encoder is
selected, the bits each register holds, the bit writer's count, the status
at rest. It encodes nothing.

The proof (`selftest.c`, part of `scripts/check.sh`): a client of this
project's own, built as a shared object like the library, touches the
registers in every way a driver does (plain loads and stores, the same
value twice, one byte of a register, two registers in one instruction, a
field of a register, a copy over four registers, a thousand accesses in a
loop, an encode waited for and one polled for). All 2035 accesses came out
of the trace complete and in order on the development host. The same proof
on the board, which adds the breakpoint and the bit field (2037 accesses),
is the first step of every session there.

`ve_trace_report.py` reads a trace: calls, buffers, accesses by register
name, the bytes the bit writer was given, the registers' state at every
start of an encode, and for every value written whether it names a place in
a buffer under one of the three forms this engine's blocks are known to use.

## The H616's register description

Shown on the board so far (V1). Everything else is still the hypothesis of
the maps above.

- The encoder is selected by bits 7 (encoder) and 6 (input stage) of the
  mode register `0x000`, with the decoder field at 7, after a pulse of bit
  24 of the reset register `0x004`. The mode register reads `0xc0000007`
  after power-on and `0xc00000c7` with the encoder selected.
- `0x004` reads `0x00000300` when idle.
- `0x0f0` reads zero; `0x0e0` = `0x00033010` and `0x0e4` = `0x00012011`.
  The vendor's keeper for the H6 reads the same three registers and calls
  the first the decoder's and the second the encoder's identification; the
  H6 has `0x00031010` and `0x00012010`. This engine's encoder is one
  revision on from the H6's.
- The registers of `0xa00` and `0xb00` and their writable bits are those of
  the table of V1.5. They read zero, and cannot be written, while the
  encoder is not selected.
- The bit writer: bit 9 of the status register `0xb1c` says it is ready;
  the bits go to `0xb20` and `0xb18` is written with 1 and the count from
  bit 8; `0xb90` counts the bits written; bit 31 of `0xb04` switches
  emulation prevention off. The status register reads `0x00000260` at rest.
- **Where the engine writes its stream is not shown yet.** With the start
  register `0xb80` naming a lab buffer, the bit writer's bytes went to
  physical `0x41000000` (V2.1). The candidate: the register counts in units
  of 256 bytes ("V2.1 afterwards"). Until that is confirmed without a
  write, nothing that makes the engine write may be run.
