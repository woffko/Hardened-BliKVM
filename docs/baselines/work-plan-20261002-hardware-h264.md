# Work plan: hardware H.264 encoding on the H616 (2026-10-02)

The feasibility study of 2026-10-01
(`docs/design/h616-hardware-h264-feasibility.md`) ended with "no-go for now":
no open driver exists for the H616's H.264 encoder, and the path it saw went
through a new kernel. The operator has decided to try anyway, with reverse
engineering where the open sources end. This plan fixes how the attempt is
made on the present kernel, where it stops if it fails, and what a success
has to become before it serves a viewer.

Rules as in the earlier plans: capability-gated surfaces enforced by the
server and the board owner, privileged work only in the owner or a dedicated
helper, English repository text, secrets only through `project-memory-exec`
(never in arguments, logs or receipts, and never a program on its standard
input under it), every change shipped as a signed slot or the update-runtime
package and verified on the test board, audit and design documents updated
with evidence under `evidence/private`. Additions for this plan:

- One commit per item. An item's own tests run before its commit; the full
  `scripts/check.sh` runs before the last commit of each phase and before
  every slot or package build. The branch is pushed after each phase.
- Every phase ends in a gate with numbers. A gate that is missed, or a time
  box that runs out, ends the plan there with the findings recorded. A time
  box counts the hours of work on its phase, kept in the lab log.
- The cheapest questions come first: whether a module can be built for this
  kernel at all, then whether the engine answers, then whether the open
  register maps drive it.
- The software encoder stays. Whatever this plan builds is used only when it
  works, and x264 serves whenever it does not.
- Clean room (below): open sources are read, the vendor's encoder may be
  watched at work, and nothing of the vendor's is disassembled, copied or
  shipped.
- Nothing here takes the HID lease, sends input to the target or pulses ATX.

## Where the work starts

- The software of release `0.1.0-alpha.10` (slot 65, source `4602d68`) and
  the update runtime `0.1.0~dev35`.
- The picture's way today (`docs/design/h616-software-video-encoder.md`):
  the capture device compresses 1920x1080 4:2:2 JPEG itself;
  `bli-kvm-video-encoder` decodes it with libjpeg-turbo and converts it to
  I420 (25 to 35 ms of one core per frame, on two converter threads) and
  encodes it with x264 `ultrafast`, `zerolatency`, constrained baseline, a
  key frame every two seconds and on request (28 ms per frame on three
  threads). H.264 at 1920x1080 reaches 34 to 36 frames a second and keeps
  three of the four cores busy; the unit is limited to 30 frames a second and
  250% of a core. The encoded frames go to Janus as RTP (WebRTC) and to the
  frame socket (QUIC). The bitrate, 1.5 to 8 Mbit/s, changes while the
  encoder runs.
- The test board, looked at read-only on 2026-10-02:
  - kernel `5.19.4-sunxi64`, built by the board's vendor with Arm's GCC
    8.3-2019.03. No headers package or build tree is installed; the
    kernel's own `kheaders` module is on the card (every header of the
    build, generated ones included, but no build scripts).
    `CONFIG_MODVERSIONS` is off, and module signatures are checked but not
    required;
  - the video engine is the device-tree node
    `allwinner,sun50i-h616-video-engine` at `0x1c0e000` (8 KiB of
    registers, one interrupt, clocks, a reset, an SRAM section, no IOMMU).
    The staging driver `sunxi-cedrus`, a decoder, is bound to it as
    `/dev/video0` and keeps it suspended; nothing of this project uses it;
  - 1 GiB of memory, 128 MiB of it CMA; the SoC's watchdog exists and
    systemd does not use it; gcc, make, gdb, ffmpeg and v4l2-ctl are
    installed.
- The development host has clang and the Rust toolchain for static aarch64
  binaries, Chromium and Firefox under Playwright, and neither ffmpeg nor an
  aarch64 GCC.

## What is known about the encoder, and what is not

Open sources, each under a licence that allows reading and reuse:

- jemk's `h264enc` (GPL-2.0-or-later, 2014-2015): a proof of concept for the
  A10 to H3 generations through the vendor kernel's `/dev/cedar_dev`. NV12
  or NV16 in, constant QP, I and P frames, CAVLC or CABAC; the headers are
  written through the engine's own bit writer. linux-sunxi's register guide
  documents the same block (`0xb00`, with the input stage at `0xa00`).
- Bootlin's encoder for the V3, V3s and S3 (GPL-2.0, branch
  `cedrus/h264-encoding` of `bootlin/linux`, 2023): the same block under the
  vendor's register names, with the buffers it needs and their sizes, I and
  P frames, CAVLC and CABAC, fixed QP, interrupt driven. Bootlin call the
  other generations' encoders "different (yet rather similar)".
- A cedrus patch posted on the Armbian forum in September 2025 adds JPEG and
  H.264 encoding for the 32-bit parts. Its author states that it does not
  work on the H616 and T507, whose encoders "have been updated" and, as far
  as he knows, have not been reverse-engineered.
- The vendor kernel's `cedar_ve.c` for the H616 (GPL-2.0): the kernel half
  of the vendor stack. It maps the register window into the caller, gates
  the clocks and waits for interrupts; on the H616 it finds the encoder's
  interrupt status and control at `0xb1c` and `0xb14`, where the older
  generations have them.
- The H616's brief: H.264 encoding up to 4K at 25 frames a second.

Not known from any open source: the engine's version value on the H616; how
far its encoder's registers, buffers and input formats differ from the two
maps above; its speed and its quality. An attempt to run Bootlin's encoder on
an H616 (January 2024) never got as far as the engine.

So the first question about the engine is cheap: do the open maps, or
something near them, drive it? Only if they do not does the plan turn to
watching the vendor's encoder.

## Clean room

1. Read and used: sources with an open licence (the list above, mainline
   `cedrus`, the vendor kernel's GPL driver and its headers) and public
   documentation. Register offsets and bit meanings are facts; where code is
   adapted, the file keeps the notice and the licence of its source.
2. Watched, if the operator approves it (V2R): the vendor's encoder library
   running on the test board as a black box, called through its published
   interface. What is recorded is its traffic with the hardware: which
   register it reads or writes, with which value, in which order, for which
   input. From such traces the register description is written in this
   project's own words.
3. Never: disassembling or decompiling a vendor library, reading vendor
   source that was not published under an open licence (its interface
   headers excepted, as the description of how to call it), copying any of
   it, and putting a vendor file into Git, a package, a slot or an image.
   The library lives in a cache outside the repository and on the board's
   tmpfs for the length of a session.
4. `docs/licensing.md` records this rule and the sources. The product
   contains only code written here or taken from open sources under their
   licences.

## Design

Each decision below may be changed in the design notes with the reason
recorded: `docs/design/h616-ve-h264-encoder.md` (the engine: sources,
register description, lab record) and
`docs/design/h616-ve-encoder-driver-v1.md` (the driver).

### Two stages, two shapes

- **Research (V1 to V3)**: a lab module in the kernel hands the engine to a
  lab tool in user space, the way the vendor's kernel hands it to the
  vendor's library: the registers mapped into the tool, buffers with their
  bus addresses, a wait for the interrupt. Register sequences then change
  in seconds, without a kernel rebuild, and the same arrangement can carry
  the vendor library if it comes to that. Whoever programs the engine can
  reach any memory, so this pair is a lab instrument, loaded for a session
  and never part of a package, a slot or an image.
- **The product (V4 to V7)**: a driver that keeps the registers and the
  buffers to itself and offers a closed set of requests, and an engine in
  `bli-kvm-video-encoder` that uses it. Only this shape keeps the boundary
  the feasibility study asked for.

Both modules are built from one shared part (probe, clocks, reset, SRAM
section, interrupt) and differ in what they offer to user space.

### A module for this kernel

The board's kernel was built by its vendor and has no build tree. The recipe
(`scripts/build-ve-module.sh`) puts one together from pinned parts: the
source of Linux 5.19.4 for the build scripts, the board's configuration
(`/proc/config.gz`), every header from the kernel's own `kheaders` module in
place of the upstream ones, and the compiler the kernel was built with. The
generated configuration header of the rebuilt tree must define exactly what
the one in `kheaders` defines; that is the check that configuration and
compiler match. Without symbol versions a module needs no more than that to
load, and it loads unsigned, which the kernel notes in its taint flags.

### The lab module and the lab tool

Module `hardened-blikvm-ve-lab` (GPL-2.0-only), root only:

- binds to the engine's node after `cedrus` has been unbound for the
  session, and takes the clocks, the reset, the SRAM section and the
  interrupt as `cedrus` does; `cedrus` is bound again afterwards;
- maps the register window into its one client;
- allocates DMA buffers for it, maps them, and tells their bus addresses.
  They are uncached mappings, so neither side sees stale data;
- waits for the encoder's interrupt as the vendor kernel's driver does
  (status at `0xb1c`, enables at `0xb14`). An interrupt it cannot attribute
  disables the line and ends the session;
- resets the engine and frees everything when the client closes it or dies.

Crate `bli-kvm-ve-lab`, one static aarch64 binary, copied to the board's
tmpfs for a session:

- **Headers.** SPS, PPS and slice headers are written by the tool, the slice
  header through the engine's bit writer as both open maps do.
- **Input.** Test pictures come from a generator in the tool (text that
  scrolls, a window dragged over a still desktop, full motion, a still
  picture with a moving pointer), the same on the board and on the host, so
  no screen of the operator is recorded. Sizes: 1920x1080, 1280x720,
  1366x768.
- **Judging a stream.** The board's ffmpeg decodes it and the tool compares
  the result with the source (PSNR); on the host Chromium decodes it with
  WebCodecs. x264 with the product's settings, driven through the encoder
  crate's own wrapper, is the reference for quality and speed.
- **A software stand-in** for the engine (I_PCM and skipped macroblocks
  behind the same headers) makes the tool's own parts, and later the
  encoder's hardware path, testable on the host.

### Rules for experiments on the board

1. The engine is programmed only while nobody holds the HID lease; otherwise
   the run waits. The operator can lift this in the goal text.
2. Before any trigger every address register the maps name holds an address
   inside the session's own buffers, and the bitstream's end register bounds
   what the engine may write. An address of zero is not memory on this SoC.
3. First runs use a small picture in buffers many times its size, with
   marker bytes around the part in use; markers are checked after every run.
4. A keeper process arms the SoC's watchdog (16 s) for the length of a run:
   a kernel that hangs is reset instead of waiting for a hand. `sync`
   precedes every run. The engine is reset before the watchdog is disarmed.
5. The mask probe (V1.5) writes no trigger. Registers outside the blocks the
   open maps name are read, never written.
6. Afterwards: the lab module unloaded, `cedrus` bound and suspended as
   found, the watchdog disarmed, no file left on the board, no unit or
   process. The kernel log is read after every session; a warning it did
   not have before ends the session.
7. A hang or reboot that is not understood stops the work on the board
   until it is explained in the lab record; a second one ends the plan.

### The driver

Module `hardened-blikvm-ve`, GPL-2.0-only, in `kernel/hardened-blikvm-ve/`,
built for exactly the board's kernel release.

- It binds to the video engine's node in place of `cedrus`, which the
  product then no longer loads.
- It allocates every buffer the engine sees (input picture, bitstream,
  reconstruction, the engine's work buffers) for the size that was asked,
  and programs only their addresses. A client never supplies an address or
  a register value, and never sees the registers.
- One device node, one client at a time. A small closed set of requests:
  configure (size, entropy mode), encode one frame (key or not, QP, the
  header bits to put before the slice data, bounded in length), read the
  result (bytes, status); the input picture and the bitstream are mapped
  into the client. Every field is checked against a list or a range before
  it becomes a register field.
- An encode that does not finish in time resets the engine and reports it.
  Closing the node stops the engine and frees its buffers.
- Rate control and the stream's headers are the client's: they change with
  the product, and a wrong header costs a picture, not memory.
- No IOMMU confines the engine on this kernel, so the driver's checks are
  the confinement; the design note says so in its threat model. A kernel
  with the H616's IOMMU would add a second wall and is not part of this
  plan.
- Whether the node is a device of its own (as assumed here: least code in
  the kernel) or a V4L2 encoder is decided in V4.1.

### In the encoder

- `bli-kvm-video-encoder` gains a third engine, hardware H.264, behind the
  one it has for x264: NV12 instead of I420 from the converter, a
  frame-level rate controller for the bitrates of today, key frames on
  request and every two seconds, SPS and PPS before every key frame,
  constrained baseline. What leaves the encoder (RTP for Janus, the frame
  socket's records) keeps its form.
- It is chosen when the node opens and a test frame encodes, and given up
  for x264, with a line in the journal, when it fails at the start or
  repeatedly while running. `GET /api/vm/screen` says which one runs.
- The unit may open exactly this node. Its frame-rate limit and converter
  threads are set for the hardware path from V3's numbers.

## Phases

### V0: preparation (nothing on the board is touched)

- V0.1 `docs/design/h616-ve-h264-encoder.md`: the sources with their
  licences and what is taken from each; the clean-room rules;
  `docs/licensing.md` gains them. The two open register maps side by side
  as the hypotheses to test: offsets, fields, the order of an encode, the
  buffers and their sizes.
- V0.2 The same note: the lab's method and rules as above, the test
  sequences, the measures, the form of a receipt and of the lab log.
- V0.3 The lab tool's host part: the picture generator, the header writer,
  the software stand-in, the register plan for both maps against a recorded
  register file, PSNR. Tests: Chromium decodes the stand-in's streams at
  every size on the host; the plans of both maps are compared with tables
  written from the sources.
- V0.4 `check.sh` runs these tests; a packaging test asserts that neither
  the lab tool nor the lab module is in a bundle, a package or an image
  list.

### V1: a module for this kernel, and first contact (at most 12 hours; gate)

- V1.1 The build recipe as designed. An empty module, and one that
  registers a platform driver and a device node, load and unload on the
  board without a line of complaint beyond the taint notice.
- V1.2 The lab module, with the shared part.
- V1.3 A session: `cedrus` unbound, the lab module bound, the engine's
  version and the readable registers of the general block and of `0xa00`,
  `0xb00` and `0xe00` read; everything given back. Read only.
- V1.4 Buffers: allocated, patterns written and read through the mappings,
  their addresses within the board's memory, freed; CMA as found.
- V1.5 With the encoder selected and nothing triggered: which bits of the
  registers the two maps name can be set and cleared. Compared with both
  maps.
- **Gate V1**: modules built by the recipe load; the version is recorded;
  the encoder block holds what is written to it; buffers work; after the
  session `cedrus` is bound as before and the kernel log is clean.
  Otherwise the plan ends: without a module there is no product, and
  without an answer from the block there is nothing to drive.

### V2: a key frame from the open maps (at most 12 hours; gate)

- V2.1 The engine's bit writer puts SPS and PPS into the bitstream buffer:
  the engine's first and smallest write to memory, and the proof that it
  takes the addresses as they are. If nothing arrives, the state of the
  SoC's IOMMU is the first thing to read.
- V2.2 One key frame at 320x240, then 1280x720 and 1920x1080, with
  Bootlin's map, with the older one, then with what V1.5 suggests. Every
  attempt is a receipt: what was written, the status, the bytes, the
  decoders' verdict.
- **Gate V2**: a 1920x1080 key frame that ffmpeg and Chromium decode, with
  a PSNR within 3 dB of x264's key frame of the same picture at the same
  QP. Reached: V3. Not reached within the time box: V2R, if the operator
  approves it; otherwise the plan ends.

### V2R: watching the vendor's encoder (only with approval; at most 20 hours; gate)

- V2R.1 One question to the operator: which library (the vendor's encoder
  for the H616, an aarch64 build), from where, and what is done with it
  (rules 2 and 3 of the clean room). Without a yes the plan ends at V2.
- V2R.2 The library gets the engine through the lab module instead of the
  vendor's kernel: a shim answers what it asks of the vendor kernel's
  engine device and allocator (request numbers and structures from the
  vendor kernel's GPL headers), or the lab tool supplies those parts where
  the library's interface lets its caller do so.
- V2R.3 The trace. The register window the library sees is closed to it.
  An access faults; the handler records offset, direction and value, opens
  the window, lets that one instruction run by a breakpoint placed behind
  it, and closes the window again. No instruction is emulated. Proven first
  on the board with a test program of this project in the library's place:
  its known accesses must come out of the trace complete and in order.
- V2R.4 The vendor's encoder under the trace: a small picture first, then
  1920x1080; one parameter changed per run (size, QP, key or not, entropy
  mode, bitrate). The rules for experiments apply.
- V2R.5 The register description written from the traces; the lab tool
  taught; the vendor's files removed from the board.
- **Gate V2R** is gate V2, with the lab tool alone and no vendor code in the
  run. Missed, or no library that runs on this board: the plan ends.

### V3: a stream (at most 16 hours; gate for the product)

- V3.1 P frames: reference and reconstruction buffers, 300 frames of every
  sequence, every frame decoded and compared; no drift (the PSNR of the last
  30 frames of a still sequence within 0.5 dB of frames 30 to 60).
- V3.2 The stream browsers get: constrained baseline, the level for the
  size and rate, cropping for 1080 and 768 lines, a key frame on request.
- V3.3 Time per frame at each size, key and P, with the CPUs idle and with
  three cores busy; frames per second sustained for ten minutes.
- V3.4 Quality: PSNR at 1.5, 2.5, 5 and 8 Mbit/s against x264 with the
  product's settings on every sequence.
- V3.5 Rate control in software: a QP per frame from the bytes of the
  frames before, and one more pass at a higher QP, with the reference left
  as it was, when a frame exceeds what the one-second buffer of today
  allows. The engine's own rate-control registers are studied only if this
  cannot hold the gate.
- V3.6 An hour of encoding without a wrong frame; a timeout and the reset
  after it; 1000 starts and stops.
- **Gate V3 (go for the product)**, at 1920x1080, all of: every frame of
  every sequence decodes in ffmpeg and Chromium; no drift; at least 30
  frames a second sustained (33 ms a frame, key or P); the rate over any ten
  seconds within 15% of the target; V3.6 passed; and PSNR not more than
  1 dB below x264's at 8 Mbit/s and not more than 2 dB below at 2.5 Mbit/s
  on every sequence. If only the quality condition is missed, the operator
  is asked once whether a free CPU is worth that picture; without a yes, and
  in every other case, **no-go**: the numbers are recorded and the plan ends
  with x264 as it is.

### V4: the driver

- V4.1 `docs/design/h616-ve-encoder-driver-v1.md`: threat model, the
  interface and its limits, buffer ownership, failure rules, power, what
  the missing IOMMU means, the licence, how it reaches a board.
- V4.2 The driver, on the shared part.
- V4.3 Tests. On the host: the request checks and the register plan as
  plain functions, with at least 100000 random requests (no register value
  outside its field, no size beyond a buffer). On the board: a hostile
  client (every request out of range, out of order, twice, closed in the
  middle, mapped beyond its size) leaves no line in the kernel log and no
  buffer behind; the lab tool's sequences through the driver.
- **Gate V4**: through the driver the streams of V3 are the same bytes as
  through the lab module for the same input and QP; V4.3 passed; unloading
  it leaves the board as it was.

### V5: the encoder

- V5.1 The hardware engine in `bli-kvm-video-encoder`: NV12 conversion, the
  rate controller of V3.5, key frames, counters in its statistics line.
- V5.2 Choice and fallback as in the design; `GET /api/vm/screen` reports
  the encoder in use.
- V5.3 The unit: access to the node and nothing else new; limits for the
  hardware path.
- V5.4 Tests on the host with the stand-in behind the driver's interface;
  the frame socket's and the QUIC picture's contracts unchanged; `check.sh`.

### V6: packaging

- V6.1 The update-runtime package's next version carries the module for
  this kernel release only, loads it at boot, gives the node to the
  encoder's group, and no longer loads `cedrus`. The capture unit must not
  depend on the capture device's number, which changes when `cedrus` is
  gone (it names `/dev/video1` today).
- V6.2 The SD image builder and the RAW system update carry it, and the
  image check refuses a module that does not match the image's kernel.
- V6.3 Licences: the module's source and build recipe, `docs/licensing.md`,
  the packaging tests (no lab tool, no lab module, no vendor file).

### V7: the board, the documents, the decision

- V7.1 Package, encoder and slot installed. The probes do not touch the
  target, so a moving picture is made in two ways. The lab's sequences are
  played as a capture stream into a second, transient instance of the
  encoder, beside the product's units: frames a second, the stages' times,
  CPU and the SoC's temperature at 30 frames a second and at the limit, for
  hardware and for x264 on the same day. And end to end with what the
  target shows, over WebRTC and over QUIC: frames a second, capture to
  drawing, CPU of every unit on the picture's way; twelve hours with a
  viewer.
- V7.2 Regression: GUI walks, VP8, MJPEG, the firewall probe; a slot from
  before this plan still shows H.264 on the new system.
- V7.3 Documents: the two design notes, the video design note, the
  feasibility note's outcome, the security boundaries, an audit note with
  every item and its evidence; the memory note.
- **Gate V7**: at 30 frames a second of a moving 1920x1080 picture the
  encoder uses at least one core less than with x264; twelve hours without
  an encoder error or a kernel message. Passed: the hardware engine stays on for H.264, at the
  frame rate V3 and V7.1 showed it can hold. Missed: it is switched off in
  the unit, x264 serves, and the numbers say why.
- V7.4 End state: every administration switch and the operator's settings
  as found; the lab tool and the lab module nowhere on the board.

## What the operator should expect

- It may end at a gate. The open maps are for other generations, and the
  one public statement about the H616's encoder says they do not fit.
  Watching the vendor's library needs a build of it that runs on this
  board, which is not certain either.
- What remains in every case: a way to build modules for this kernel, the
  engine's version, the record of what was tried, and a lab that makes the
  next attempt, or a study of the JPEG decoder, cheap.
- If it works: H.264 at 1920x1080 no longer bounded by x264, and at least
  one core free. If the engine is as fast as its brief says, the frame rate
  is then bounded by the capture (50 to 60 frames a second) and by JPEG
  decoding (25 to 35 ms of a core per frame, so three converter threads
  could carry about 50), a frame is some 15 to 20 ms younger, and the board
  runs cooler. Speed and quality per bit are not known before V3.
- From V1 on, modules built here are loaded on the test board for the
  length of a session, and the kernel shows that in its taint flags until
  the next boot. During V1 to V3 the board may hang or reboot; the watchdog
  brings it back, and a session of the operator ends when it does.
- The product stops loading `cedrus`, which nothing uses.

## Out of scope, possible follow-ups

Hardware JPEG decoding (the larger gain: it would take the picture's whole
way off the CPU); H.265; a V4L2 interface and upstreaming; a newer kernel
and the IOMMU; B frames; more than one stream at a time; the engine's own
rate control beyond what V3.5 needs.

## Risks

| Risk | Handling |
| --- | --- |
| No loadable module for a vendor-built kernel without its build tree | gate V1, before any other effort: the kernel's own headers, the same compiler, the configuration header compared, no symbol versions to match |
| The open maps do not drive this engine | gate V2, then V2R |
| No vendor library that runs on this board, or no approval | the plan ends at V2 with its findings |
| The engine writes outside its buffers during research | rules 2 to 4: own addresses everywhere, small pictures in large buffers with markers, the watchdog, `sync`, reset before disarming |
| An interrupt nobody clears | the lab module handles the encoder's as the vendor kernel does and disables the line for any other |
| The operator is at work on the board | rule 1 |
| The encoder is slower or worse than x264 | gate V3; x264 stays |
| A driver bug reaches memory, with no IOMMU behind it | no address or register value from a client, closed requests, random and hostile tests, engine reset on every failure; stated in the threat model |
| The capture breaks when `cedrus` no longer creates `/dev/video0` | V6.1 |
| A slot rolled back below this plan | the encoder belongs to the system and serves as before; the older page only lacks the line that says which encoder runs |
| Licences | open sources with their notices; the modules GPL-2.0-only as works of their own beside the GPL-3.0-only application; no vendor file anywhere |

## Operator

Decided in this plan, to be changed by saying so: research on the present
kernel, with a lab module; no kernel upgrade; the product as an out-of-tree
module with a device of its own; constrained baseline; x264 kept as the
fallback; the hardware engine on by default only after gate V7.

Needed from the operator before the start:

- that modules built here may be loaded on the test board, the lab module
  among them, which gives a root process the engine; and that the board may
  hang or reboot during V1 to V3;
- these downloads to the development host, into a cache outside the
  repository: the branch `cedrus/h264-encoding` of `bootlin/linux` and
  `jemk/cedrus` (to read), Linux 5.19.4 from kernel.org with its signature,
  and Arm's GNU toolchain 8.3-2019.03 for aarch64;
- that the package, the encoder and the slots may be installed (V7).

Asked only if it comes to that: whether the vendor's encoder library may be
fetched and watched (V2R), and whether a lower quality is acceptable (gate
V3).

Left to the operator at the end: a look at the picture with their own eyes
at both ends of the bitrate range, and whether hardware H.264 stays the
default.
