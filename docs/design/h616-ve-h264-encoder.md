# The H616 video engine's H.264 encoder: sources, register maps, lab method

Plan: `docs/baselines/work-plan-20261002-hardware-h264.md`. This note holds
what is known about the encoder from open sources, the two register maps
that are the hypotheses to test, how the lab works, and, as the work goes
on, the lab's record and the register description that comes out of it.

Status (2026-10-02, gate V2 passed): the engine encodes key frames. A
1920x1080 key frame takes it 7 ms, decodes in ffmpeg and Chromium, and is
2 to 3 dB below x264's at the same QP. The way there: in V2.1 the engine
wrote its bits into the kernel's memory instead of the buffer it was given;
the encoder block of this generation takes its addresses in units of 256
bytes, which the vendor's encoder library, watched on the board without
the engine, showed for the two parts whose encoder is this one's
predecessor. The lab tool has that form as its third hypothesis (`h6`).
Next is V3: P frames and a stream.

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
   Since V2R the lab's run script does this itself, and also copies the
   kernel's own image (its instructions and read-only data) before the
   engine is handed to the lab module and compares it word by word after
   the engine is back. The kernel changes its own instructions while it
   runs: a static key that is switched turns single instructions from "no
   operation" into a branch or back. Such words are counted and held
   against nobody; any other word that differs is a write that went astray,
   and the comparison says where (see "V3.1" for the session that taught
   this).

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

**V2R.4, the vendor's encoder under the trace.** Four sessions on the board
on 2026-10-02, after the operator's power cycle (the board was first read:
it had come up as it was before the incident, with the taint flags back at
1024). Each began with the trace's own test. No session left a line in the
kernel's log, a module, a file or a process; `cedrus` stayed bound and
suspended.

| Session | Runs | What came out |
| --- | --- | --- |
| `ve-trace-20261002-selftest` | the trace's own test | 2037 accesses complete and in order: the breakpoint works on the board |
| `-first` | the library told this engine's identification | it reads `0x0f0`, finds zero, reads `0x0e4`, and makes no encoder: this build (its own log line says branch CedarC-v1.1.9, May 2017) does not know encoder `0x00012011` |
| `-versions` | six identifications | it takes the H6's (`0x0e4` = `0x00012010`), the H5's (`0x0f0` = `0x1718` in the upper half), the V3s' (`0x1681`) and the H3's (`0x1680`), and encodes three frames with about 440 register accesses. Its log names two paths: the H5 and the H6 go one way, the V3s and the H3 the other. Which codec number its caller asks for does not matter |
| `-h6-matrix` | the H6's run eleven times with one setting changed: the arena moved; an offset taken off addresses; 1920x1080, 1280x720, 1366x768; another QP; CABAC; high profile; its rate control on; forced key frames; its own tables fetched instead of given | below |

A run is three frames of 320x240 at a fixed QP of 30 unless its setting
says otherwise. The stand-in does not encode: the stream the library gets
back is empty, and what it does with a real stream's statistics (its rate
control) is seen only as far as the registers it writes before a frame.

**V2R.5, what the traces say.** The same library drives two generations
of this encoder block, and tells them apart by the engine's
identification:

| | V3s, H3 | H5, H6 |
| --- | --- | --- |
| Every address register of the encoder block (`0xb80`, `0xb84`, `0xba0` to `0xbc0`) | the address as it is | the address divided by 256 |
| The input stage's addresses (`0xa78`, `0xa7c`, `0xa80`) | as they are | as they are |
| `0xa00`, `0xa2c`: the picture's size | in macroblocks | the visible size in units of 8 pixels, rounded up (1920x1080: 240 by 135; 1366x768: 171 by 96) |
| `0xb08` for QP 30, a stride of 20 macroblocks | `0x0004051e`: the chroma QP offset 4 at 18:16 | `0x0800051e`: bit 27, no chroma QP offset |
| `chroma_qp_index_offset` in its PPS | 4 | 0 |
| `0xb10` | `0x00010004` (Bootlin's value) | `0x00011000` |
| `0xb2c`, `0xb40` | not written at a fixed QP | written 0 |

For the V3s it writes what Bootlin's driver writes, which makes the trace a
check of itself. Moving the arena moved every address register by the same
amount in its form (`0x6b...` to `0x53...`), so the forms are not chance.
With an offset told by our allocator, the library gives the engine what the
allocator calls the engine's address; the published keeper of the H6 tells
none.

The H616's encoder (`0x00012011`) is one revision on from the H6's
(`0x00012010`), which the library drives exactly as the H5's. That, and
where the bits of V2.1 went, is why the form of the H5 and the H6 is the
lab's hypothesis for this engine from here on (`h6`); the description of
that form is in "The H616's register description" below.

The lab tool was taught: `Hypothesis::H6` writes, register for register and
in the same order, what the library wrote for the H6 (tests in `regs.rs`
hold the traced values of a key frame and a P frame at 320x240 and the
size-dependent ones at 1280x720, 1366x768 and 1920x1080); its software
stand-in has the newer generation's forms too, and Chromium decodes the
stream it makes through this plan (`tools/playwright/ve_standin_contract.mjs`).

The explanation of V2.1 is thereby confirmed by a means that made the
engine write nothing (rule 8), and by rule 7 the work on the board goes
on. The vendor's files were on the board's tmpfs for the length of each
session only.

Receipts: `evidence/private/ve-trace-20261002-selftest`, `-first`,
`-versions`, `-h6-matrix` (each run: the trace, what was asked and what
came back, the library's own messages, the report).

### V2.1 again, 2026-10-02: the bit writer's bytes are in the buffer

With the stream buffer described in units of 256 bytes (`0xb80` =
`0x00774100` for the buffer at `0x77410000`, `0xb84` = `0x007780ff`,
`0xb8c` = `0x02000000`), and nothing else changed against the run that
went astray:

- SPS and PPS of a 320x240 stream, once: the 25 bytes are at the beginning
  of the stream buffer, the rest of it is zero, `0xb90` reports 200 bits.
- 700 times over: all 17500 bytes are there. The align trigger after the
  last put (the library's way) hands the last portion to memory.
- The same with the mode register's memory field at 2 (the H6 keeper's
  value) and at 3 with bit 20 (what `cedrus` sets): no difference for the
  bit writer. The mode register holds both (`0xc00200c7`, `0xc01300c7`).

Rule 9 at work, in the run script from here on: the digest of the kernel's
image was the same before and after every session (283 pieces), and the
scan of all memory found the parameter sets at one place only, the stream
buffer. (In the sessions that follow it also finds the first page of every
stream file the session itself left on tmpfs.)

Receipts: `evidence/private/ve-lab-v2-20261002-probe-after-reset`,
`-bits-h6-a`, `-bits-h6-b`.

### V2.2, 2026-10-02: key frames

The first encode: one key frame of `drag` at 320x240, QP 30, by the plan of
`h6`. The engine finished with status 1 after 0.3 ms and reported 43368
bits; the 5421 bytes decode in Chromium to 40.05 dB against the picture.

Then one key frame of every sequence at each size, QP 30, each beside
x264's key frame of the same picture at the same QP. The board's ffmpeg
decoded every stream of the engine without a word of complaint and to the
visible size (cropping is right at 1366x768 and 1920x1080); Chromium
decoded the 1920x1080 ones to the same PSNR.

| Size | Sequence | Engine, bytes | x264, bytes | Engine, dB | x264, dB | Difference | Engine, ms |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 320x240 | scroll | 23806 | 25414 | 33.91 | 36.39 | -2.48 | 0.3 |
| 320x240 | drag | 5421 | 6656 | 40.05 | 41.99 | -1.94 | 0.3 |
| 320x240 | motion | 3347 | 3604 | 35.81 | 38.89 | -3.08 | 0.3 |
| 320x240 | pointer | 5421 | 6689 | 40.03 | 41.87 | -1.85 | 0.3 |
| 1280x720 | scroll | 214718 | 226827 | 35.04 | 37.35 | -2.31 | 3.1 |
| 1280x720 | drag | 69146 | 73201 | 39.78 | 42.15 | -2.37 | 3.1 |
| 1280x720 | motion | 39138 | 35043 | 35.76 | 38.68 | -2.92 | 3.0 |
| 1280x720 | pointer | 69535 | 73566 | 39.63 | 42.16 | -2.54 | 3.1 |
| 1366x768 | scroll | 234996 | 248190 | 35.20 | 37.48 | -2.28 | 3.6 |
| 1366x768 | drag | 78815 | 83197 | 39.80 | 42.18 | -2.39 | 3.6 |
| 1366x768 | motion | 44772 | 40005 | 35.75 | 38.69 | -2.94 | 3.4 |
| 1366x768 | pointer | 87889 | 90364 | 39.46 | 41.79 | -2.33 | 3.6 |
| 1920x1080 | scroll | 344737 | 364204 | 36.48 | 38.69 | -2.21 | 7.0 |
| 1920x1080 | drag | 145746 | 153192 | 40.09 | 42.50 | -2.41 | 7.0 |
| 1920x1080 | motion | 88663 | 78559 | 35.75 | 38.69 | -2.94 | 6.7 |
| 1920x1080 | pointer | 146531 | 153812 | 39.97 | 42.49 | -2.52 | 7.0 |

What this says:

- The form of the H5 and the H6 is this engine's: everything the plan of
  `h6` writes for a key frame is taken as the library's traces say.
- A 1920x1080 key frame takes the engine 7 ms, from the start trigger to
  the interrupt. x264 on this board takes 28 to 47 ms for a frame.
- At the same QP the engine's picture is 2 to 3 dB below x264's and its
  frame mostly a little smaller: it quantises more coarsely than x264 does
  at the same number. What that means at the same rate is V3.4's question.

Two sentences of the plan were not followed, for these reasons:

| The plan | Done | Why |
| --- | --- | --- |
| V2.2: with Bootlin's map, with the older one, then with what V1.5 suggests | only the form that the traces gave | the two maps write the encoder block's addresses as they are, and this engine takes them for 256 times that: an encode by either would write to places nobody chose (rule 8) |
| The reference is x264 driven through the encoder crate's own wrapper | for a fixed QP: libx264 through the board's ffmpeg, with the product's preset, tune and profile, and a constant QP for every frame type | the product's wrapper sets a bitrate and has no constant QP; the comparisons at a bitrate (V3.4) use the wrapper as planned |

Receipts: `evidence/private/ve-lab-v2-20261002-key-h6-a`, `-key-h6-b`,
`-key-720p-768p-qp30`, `-key-1080p-qp30` (the streams of both encoders,
the registers written, the status and time of every frame, ffmpeg's and
Chromium's verdicts, the PSNR).

### Gate V2: passed

A 1920x1080 key frame of every sequence decodes in the board's ffmpeg and
in Chromium, and its PSNR is within 3 dB of x264's key frame of the same
picture at the same QP: 2.21, 2.41, 2.94 and 2.52 dB below. The run used
the lab tool alone; no vendor code was on the board. After every session
`cedrus` was bound and suspended, the kernel had said only the lab
module's and the decoder's own lines, and the digest of its image was
unchanged.

### V3.1, 2026-10-02: P frames, and a false alarm of rule 9

The lab tool got its long run (`stream`): frames at a fixed QP or at a
bitrate, a frame encoded once more when the rate control asks for it, a
loop that can be paced, pictures drawn once and repeated, and a timeout
that is provoked. Its pictures are drawn by all processors now; they are
the same pictures (the fingerprints in its tests did not change).

The first P frames: `drag` and `motion` at 320x240, a key frame and four P
frames at QP 30. The plan of `h6` for a P frame (the reference is the
frame before, in the other of two sets of buffers; `0xb04` = `0x410`)
worked at once: ffmpeg decodes all five frames.

Then 300 frames of every sequence at 1920x1080 and QP 30, one key frame
and 299 P frames, every frame decoded by the board's ffmpeg and compared
with its source:

| Sequence | Frames decoded | ffmpeg's complaints | Mean PSNR, dB | Frames 30 to 60 | Last 30 | kbit/s at 30 frames a second | Encode, ms (mean, longest) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| scroll | 300 | none | 36.32 | 36.88 | 36.10 | 1700 | 7.2, 7.5 |
| drag | 300 | none | 41.95 | 41.83 | 42.09 | 1998 | 7.1, 7.2 |
| motion | 300 | none | 33.36 | 33.36 | 33.36 | 37086 | 7.4, 7.8 |
| pointer | 300 | none | 41.37 | 41.46 | 41.33 | 379 | 7.0, 7.4 |

No drift: on the still sequence (`pointer`) the last 30 frames are 0.13 dB
from frames 30 to 60, after 299 frames that each refer to the one before.
`scroll` shows other text at its end than at its beginning, which is what
its 0.8 dB are.

What the loop costs besides the encode, measured on the way: the buffers
the lab module gives are not cached for the processor. Writing a picture
into one is fast (3 MB in about 3 ms). Reading is slow, about 60 MB/s even
in one run and 6 MB/s byte by byte: reading the guards of every buffer
after every frame took 175 ms, 25 times the encode. The lab now copies a
guard out in one run before it compares it (15 ms for all of them) and can
be told to do so every so many frames. With the guards read every 30
frames, pictures drawn in advance and no pacing, the loop makes 88 frames
a second at 1920x1080 (11.4 ms a frame: picture in, about 250 register
accesses, 7.2 ms of encoding, stream out). For the driver this means: the
stream must not be read out of uncached memory.

**The false alarm.** A session of 13 minutes ended with status 79: six of
the 283 pieces of the kernel's image had another digest than before it.
Read without touching the engine again, the six pieces differ from the
kernel's image file in 24 single instruction words, every one of them "no
operation" where the file has a branch or a branch where the file has "no
operation", in `netif_rx_internal`, `netif_receive_skb`,
`__netif_receive_skb_core`, `dev_queue_xmit_nit`, `packet_rcv`,
`ip_output`, `ip6_output` and their like. Those are the places where the
network stack asks its static keys (time stamps for a socket that wants
them, programs attached to a control group): the kernel had switched keys
while the session ran, and rewrites its own instructions when it does.
Nothing had been written by the engine. The claim of V2.1's second record,
that the digest does not change by itself, held for the ten seconds it was
tested for and is wrong for a quarter of an hour. Rule 9's check now
compares words and knows this one kind of change (above).

Receipts: `evidence/private/ve-lab-v3-20261002-p-first`, `-stream-try`,
`-p300-qp30`, `-time-idle`, `-time-guards`, `-rates-a` (the alarm),
`-low-qp`.

### V3.5, 2026-10-02: rate control in software

The engine is given a QP for a frame and nothing else; the stream's rate
is steered by the lab tool (`rate.rs`), as the product's encoder will do
it:

- The bound is the product's of today: a leaky bucket of one second at
  the target rate, which x264 is given as its buffer.
- A frame's QP follows from the bytes of the frames before: a frame that
  cost more than its share raises the QP of the next (by at most 3), one
  that cost less lowers it (by at most 1). A frame's share is the average,
  a little more while the bucket is below three tenths, less above. Every
  QP from 0 to 51 is used.
- A key frame's QP is planned from the last key frame's bytes into what
  the bucket has room for.
- A frame that does not fit the bucket is encoded once more at the QP its
  size calls for, against the same reference: the engine has two sets of
  buffers, and a pass that is thrown away has written only the frame's own.
- A frame that does not fit then either is left out: the stream gets a
  frame of skipped macroblocks in its place (ten bytes at 1920x1080, made
  in software), which a decoder shows as the picture before it. The engine
  cannot quantise more coarsely than QP 51, and on the hardest sequence
  that is 3 Mbit/s: at 1.5 and 2.5 Mbit/s the first runs of V3.4 made twice
  and 1.24 times their rate, with 148 and 146 frames beyond the bucket.
  Leaving frames out is what holds the rate there, at the price of a
  picture that moves in fewer steps.

Tests on the build host run the controller against a model of an encoder
(bits halve with six steps of QP): a steady picture is held within 5% in
every ten seconds at 1.5 to 8 Mbit/s; a change of scene to ten times the
cost overruns nothing; a key frame every second fits; a picture that fits
at no QP is held within 12% by leaving frames out. The numbers on the
board are V3.4's.

## The H616's register description

Shown on the board so far (V1, V2.1), and the hypothesis for the rest: the
form in which the vendor's library drives the H5 and the H6 (V2R.5). What
an experiment confirms on this engine moves from the second list to the
first.

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
- The stream start register `0xb80` counts in units of 256 bytes: written
  with a buffer's address as it is, the bit writer's bytes went to that
  value shifted left by eight bits (V2.1); written with the address divided
  by 256, they are in the buffer (V2.1 again).
- A key frame by the hypothesis below works at four sizes (V2.2): the
  input stage's and the encoder block's registers of a key frame are as
  the table says, the engine's reset and select before every frame
  included. The interrupt comes, the status reads 1, `0xb90` holds the
  length in bits, and `0xb50`, `0xb54` and `0xb58` hold statistics of the
  frame.
- The mode register's memory field does not matter to the bit writer; key
  frames were encoded with it at 2.
- A P frame by the hypothesis below works (V3.1): the reference is the
  reconstruction of the frame before, in the other of two sets of buffers,
  and `0xb04` holds `0x410`. 299 P frames in a row do not drift.
- Every QP from 0 to 51 encodes; a 1920x1080 frame takes the engine 7.0 to
  7.8 ms, key or P, whatever its QP and its size in bytes.

The hypothesis `h6`, from the vendor's library for the H5 and the H6. Per
stream, the parameter sets go through the bit writer into the stream
buffer and are read back from it; per frame the library has its keeper
reset the engine and select the encoder, and then writes, in this order:

| Step | Registers | Values |
| --- | --- | --- |
| Input stage | `0xa00` | the visible width in units of 8 pixels at 31:16, the height at 15:0, both rounded up |
| | `0xa04`, `0xa14` | the input stride in macroblocks, at 31:16 and at 15:0 |
| | `0xa2c` | height at 26:16 and width at 10:0, in the same units of 8 pixels |
| | `0xa08` | 0: NV12 |
| | `0xa78`, `0xa7c`, `0xa80` | luma, chroma, and where a second chroma plane would be (chroma plus a quarter of the luma's size), as they are |
| Interrupts, status | `0xb14`, `0xb1c` | read, and written back with `0xf` and with 7 set |
| First parameters | `0xb08` | read, and written with the QP at 5:0, bit 8 (dynamic motion estimation) and the stride in macroblocks divided by 48, rounded up, at 13:10 |
| | `0xb48`, `0xb4c` | thresholds that grow with the stride S in macroblocks: `(6S/10) << 16 \| S/10` and `(32S/10) << 16 \| 26S/10` |
| | `0xb40`, `0xb2c` | 0 at a fixed QP |
| Stream | `0xb88` | the bit offset at which this frame begins in the buffer (0 for the first; the library uses one large buffer for many frames) |
| | `0xb80`, `0xb84` | start and last byte, divided by 256 |
| | `0xb8c` | the offset plus the room in bits |
| Buffers | `0xba0`, `0xba4` | reference luma and chroma, divided by 256; a key frame names its own reconstruction |
| | `0xbb0`, `0xbb4` | reconstruction luma and chroma |
| | `0xbb8`, `0xbbc` | the sub-pixel buffer of the last frame and of this one |
| | `0xbc0` | macroblock info |
| | `0xbc4`, `0xb28` | 0 |
| Headers | `0xb04`, `0xb20`, `0xb18` | bit 31 of `0xb04` set for the four bytes of the start code and cleared for the unit's header byte and the slice header; before every put a read of `0xb1c` for bit 9 |
| Last parameters | `0xb04` | 0 for a key frame, `0x410` for a P frame (bit 4, and bit 10, which no map explains); bit 8 more with CABAC |
| | `0xb08` | bit 27 added to what it holds |
| | `0xb0c`, `0xb24`, `0xb60` | 0 |
| | `0xb10` | `0x00011000` |
| Start | `0xb18` | 8 |
| Afterwards | `0xb90`, `0xb1c`, `0xb50`, `0xb54`, `0xb58` | the length in bits; the status, written back to clear it; three statistics |

- Headers that stand alone (the parameter sets at the beginning) end with
  trigger 2 after the alignment bits; only then does the library read them
  from the buffer. In V2.1 the last bytes of the bit writer's output were
  not in memory; this trigger may be what hands them over.
- The buffers' sizes are those of the V3s map: the library asks its
  allocator for reconstruction and sub-pixel buffers of exactly Bootlin's
  sizes at every picture size tried (3317760 and 51040 bytes at
  1920x1080). Its macroblock info buffer is 128 bytes for every macroblock
  of the width (15360), a little less than Bootlin's; the lab's is larger
  than both.
- The library's own headers: baseline profile without constraint flags,
  picture order type 0 with an 8-bit counter, `frame_num` of 8 bits, no
  VUI, deblocking controlled per slice and left on. The lab keeps its own
  (the engine is given their bits either way); what it must share with the
  library's is what the engine's slice data depends on: the entropy mode,
  the QP as a difference from 26, no chroma QP offset, one reference.
- With its rate control on, the library still gives every frame a QP of
  its own at `0xb08`, and adds `0xb2c` (bit 31 for a key frame, bits 31 and
  30 with a number that changes from frame to frame for the others),
  three threshold registers `0xb30` to `0xb38`, and an address register
  the maps do not have, `0xb9c` (divided by 256 like the others), for a
  buffer of 12 bytes for every row of macroblocks and 500 more. The lab
  does not use it: a frame is given its QP.
- Main and high profile change only its headers; CABAC is bit 8 of `0xb04`.
- Not known: what bit 27 of `0xb08`, bit 10 of `0xb04` and bit 12 of
  `0xb10` mean; what the keeper writes into the mode register for the
  encoder on this part (its memory interface field: the published keeper
  sets 2 on the H6, `cedrus` sets 3 and bit 20 for this board's decoders,
  and V2.1 ran with what the reset leaves, 0). The lab tool has all three
  as variants of the hypothesis.
