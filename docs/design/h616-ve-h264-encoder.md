# The H616 video engine's H.264 encoder: sources, register maps, lab method

Plan: `docs/baselines/work-plan-20261002-hardware-h264.md`. This note holds
what is known about the encoder from open sources, the two register maps
that are the hypotheses to test, how the lab works, and, as the work goes
on, the lab's record and the register description that comes out of it.

Status (2026-10-02, V1): the engine answers and its encoder block is the
newer map's; nothing has been encoded yet.

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

## The H616's register description

Nothing yet: written from V1 on.
