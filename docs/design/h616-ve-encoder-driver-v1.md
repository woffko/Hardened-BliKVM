# The H616 video engine's encoder driver, version 1

Plan: `docs/baselines/work-plan-20261002-hardware-h264.md`, phase V4. What
is known about the engine, and how it was learnt, is in
`docs/design/h616-ve-h264-encoder.md`; this note is about the kernel module
that gives the product's encoder the engine's H.264 encoder and nothing
else of the engine.

Status (2026-10-03, gate V4 passed): the driver gives, byte for byte, the
streams the lab module gives, and a hostile client leaves nothing behind
(Record). The product's encoder on it is V5, its package V6.

## Why a driver of our own

The board's kernel has a driver for this engine, `cedrus`, which decodes
and cannot encode. No open driver encodes on this generation of the
engine. The lab module of the plan hands the whole engine to a root
process, registers and memory addresses included; that is right for a lab
and wrong for a product: the engine has no IOMMU in front of it on this
board, so whoever writes its address registers decides which memory it
reads and writes.

The driver therefore keeps every register to itself. Its client says what
it wants encoded; the driver decides what is written into the engine.

## The parties

- **The client**: the product's encoder unit, an unprivileged service that
  decodes JPEG pictures from the capture device. It is the part of the
  product most exposed to data it did not make, and the driver assumes it
  may be hostile: everything it sends is checked, and nothing it sends is
  an address or a register value.
- **The driver**: `hardened-blikvm-ve-enc`, in the kernel, GPL-2.0-only.
- **The engine**: reads and writes memory at the addresses in its
  registers, for as long as its clocks run and its reset is released.

## What the driver promises

1. The engine reads and writes only memory the driver allocated for the
   open device, for as long as that memory is allocated.
2. No request, in any order, of any size, repeated or abandoned, makes the
   kernel write outside its own buffers, hang, leak memory or log a
   warning.
3. What the client gets back is the stream of the pictures it gave: no
   byte that the client did not send in or the engine did not make from
   the client's pictures. In particular no frame is encoded against a
   reference the client's own pictures did not make.
4. When the device is closed, or its client dies, the engine is in reset
   with its clocks off before the first buffer is freed.

## The interface

A character device, `/dev/hardened-blikvm-ve-enc`, one client at a time
(a second open fails with `EBUSY`). There is no mapping: `mmap` is not
offered. There are three requests (`kernel/hardened-blikvm-ve/ve_enc_abi.h`):

| Request | What it does |
| --- | --- |
| `VE_ENC_IOC_INFO` | The interface's version, the engine's two identifications, the largest and the smallest picture. |
| `VE_ENC_IOC_CONFIGURE` | A stream begins: the visible width and height (even numbers, from 256 by 64 to 1920 by 1088), the entropy mode. The driver allocates every buffer for that size and answers how many bytes a picture has and how many a frame can have. A second configure frees and allocates again. |
| `VE_ENC_IOC_ENCODE` | One pass of the engine over a picture. The client gives the picture (NV12, whole macroblocks, the visible picture's edge repeated into the padding), the QP (0 to 51), whether it is a key frame, which of the engine's two searches for motion to use (`VE_ENC_WIDE` for the other one), the bits of the frame's headers, and where the stream of the frame is to go. The driver answers with the bytes, the time the engine took, and the engine's three statistics. |
| `VE_ENC_IOC_ENCODE` with `VE_ENC_AGAIN` | The same picture once more, at another QP or with the other search, against the same reference: for a frame that did not fit its rate, or that may be smaller with the other search. The picture is not sent again. |

**Passes and frames.** The client may make several passes over a picture
and send one of them, or none: the rate control of
`h616-ve-h264-encoder.md` (V3.5) does. The frame a new picture refers to
is therefore something the client must be able to say, and it says it by
position, never by a buffer: the last pass over the picture before (said
by nothing), the pass before that one (`VE_ENC_EARLIER`), or neither,
because no pass over that picture was sent and the stream stays where it
was (`VE_ENC_STAY`). The driver keeps the last two passes over a picture
and the frame they refer to; a third pass writes over the first.

The headers are the client's: the stream's syntax (parameter sets, slice
headers) is user space's business, and the engine does not read them. The
client gives up to three units for a frame (SPS, PPS, slice header), each
as its header byte and its payload's bits. The driver writes each behind a
start code through the engine's bit writer, with emulation prevention off
for the start code and the header byte and on for the payload, as the
vendor's library does. A unit is at most 128 bytes; the bits only ever go
into the stream buffer.

What the client cannot say: any address; any register; the buffers' sizes;
a buffer. The frame's QP, type, entropy mode and search are the only
things of the client's that reach a register, each into its own field
after a range check; the search is one bit of the motion estimation
register, which the driver writes whole.

## The frame's course

For a frame the driver, holding its lock from beginning to end:

1. checks the request against the stream's configuration;
2. copies the header units and, unless the frame is encoded again, the
   picture into its input buffer;
3. resets the engine and selects the encoder;
4. writes the registers of the frame: the plan the vendor's library
   follows for the H6's encoder, of which this engine's is the next
   revision (`h616-ve-h264-encoder.md`, "The H616's register
   description"). The plan is one plain function from the configuration,
   the buffers' addresses and the request to a list of register
   operations; the driver carries the list out;
5. starts the encode and sleeps until the encoder's interrupt or 200 ms.
   A signal does not end this sleep: the frame is done 7 ms later, and
   the engine is not reset in the middle of its work on memory for a
   client's sake;
6. reads the status and the length, and copies the frame to the client.

There are three sets of reconstruction buffers: one holds the frame the
picture refers to, and the passes over the picture are written into the
other two in turn, so that the last two passes and their reference exist
side by side. A key frame names its own set as its reference, as the
library does; the frame the stream was at before it is kept all the same,
so that a key frame that is not sent leaves the stream where it was. A
pass that is not a key frame needs a frame to refer to: if there is none
(the stream has just been configured, the last pass failed, or the
request names a pass that was not made), the request is refused with
`EINVAL`, and after a failure the client must send a key frame. That is
promise 3: the reconstruction buffers are zeroed when they are allocated,
nothing but an encode of the client's own pictures ever writes them, and
no pass reads a set that no successful pass of this stream wrote.

Which set a pass reads and which it writes is a plain function of the
request's flags and of three numbers the driver keeps (the reference, the
last pass, the pass before it), in `ve_enc_plan.c` beside the register
plan and tested with it.

## Buffers

All buffers belong to the open device and are allocated at configure:

| Buffer | Size | Kind |
| --- | --- | --- |
| Input | the coded picture: width and height in whole macroblocks, times 3/2 | pages mapped for the device; written by the processor, read by the engine |
| Stream | the picture's bytes rounded up to a whole MiB: a frame as large as its picture fits | pages mapped for the device; written by the engine, read by the processor |
| Reconstruction, three | luma and chroma by the sizes of the V3s map, in whole pages and a page more | coherent memory; the processor never touches it after the zeroing |
| Sub-pixel, three | by the size of the V3s map, in whole pages and a page more | the same |
| Macroblock info | by the size of the V3s map, and a page more | the same |

These sizes are measured, not only taken from the map
(`h616-ve-h264-encoder.md`, "V3.1 again"): the lab looked at which bytes
of its own, larger buffers the engine changed, at nine sizes from 320x240
to 1920x1088 with both searches, twice with a pattern in them that shows
a written zero too, and then gave the engine buffers of exactly these
sizes behind its guards, at eleven sizes from 256x256 to 1920x1088. The
engine writes three to five lines of luma below the picture, which the
older map's size does not hold and the V3s map's does; chroma fills the V3s
map's size to the byte at some sizes, which is why a page follows; the
sub-pixel plane and the macroblock information stay well inside. It
encodes whatever these buffers held before, so nothing depends on their
zeroing but the driver's third promise.

The engine does not finish a frame that refers to another when the
picture is narrower than 13 macroblocks (up to 192 pixels: the slice
header is written and nothing follows); key frames of any size are made.
The interface therefore begins at 256 pixels of width, where the lab has
encoded at heights from 64 to 1088.

The input and the stream are ordinary cached pages with a device mapping
(`dma_alloc_pages`), handed to the device before the engine starts and
back to the processor after it ended, because the lab measured what it
costs to read a stream out of uncached memory (60 MB/s). Every buffer is
a whole number of 1 KiB at an address that is a multiple of 1 KiB: the
encoder block's address registers count in units of 256 bytes and do not
hold their low two bits.

The stream buffer's bound is the engine's own: the start, the end and the
size registers name exactly the buffer, and a frame that does not fit
ends with the engine's "stalled" status instead of a write beyond it. The
lab has seen that: told a buffer that ends early, the engine fills it to
its last byte, writes none behind it, and reports the stall (the same
note, "V3.1 again"). Which frame does not fit is the client's to cause:
a picture of noise at QP 0 makes a frame of 1.7 times the picture's own
bytes, and at 1920x1080 already at QP 10 more than the buffer holds. The
size register counts in units of 8 KiB, which a whole number of MiB
always is. The client's own
buffer may be smaller than the frame; then nothing is copied and the
request fails with `ENOSPC` and the size the frame has.

All sizes are computed by the driver from the configured width and height,
with 64-bit arithmetic and against the limits of the interface. 96 MiB of
the board's 128 MiB of contiguous memory are the most a configuration may
take; 1920x1088 takes 15.7 MiB.

## The missing IOMMU

Nothing stands between the engine and the board's memory. So:

- every address register the engine could use during an encode is written
  before every frame, after the reset, from the driver's own table of
  buffers, in the form its block takes (the encoder block: the address
  divided by 256; the input stage: the address as it is). The registers
  the vendor's library leaves at zero are written zero: no memory lies at
  address zero on this part;
- the plan is tested on the build host with random requests: no operation
  names a register outside the plan's list, no value has a bit outside its
  register's field, and every address is inside the buffer it stands for;
- the engine is started only by the driver, and reset by the driver
  before every frame, when an encode does not end, and when the device is
  closed. A reset engine does nothing. A client cannot make the driver
  reset an engine that is encoding: its signals wait for the frame, and a
  device is closed only after its last request has returned.

What the driver cannot rule out is an engine that writes somewhere its
registers do not say. The lab has not seen that on this engine once its
address form was understood, and checks for it in every session
(`h616-ve-h264-encoder.md`, rules 8 and 9).

## Failures

| What happens | What the driver does | What the client sees |
| --- | --- | --- |
| A request is out of range, out of order, or its memory cannot be read or written | nothing reaches the engine | `EINVAL`, `EFAULT` |
| The bit writer does not become ready within 1 ms | resets the engine | `EIO` |
| The encode does not end within 200 ms | resets the engine | `ETIMEDOUT` |
| The encode ends with another status than "finished": a frame larger than the stream buffer ends "stalled" (2) | resets the engine | `EIO`, and the status |
| The engine reports a length that is not whole bytes or more than the buffer | resets the engine | `EIO` |
| A signal arrives while the engine encodes | finishes the frame; the signal is taken when the request returns | the frame |
| A signal arrives while the client waits for another request of its own to end | nothing reaches the engine | `EINTR` |
| An interrupt arrives that is not the encoder's finish | switches the interrupt line off until the next frame's reset | the frame fails with `EIO` |

After every failure of the engine, and after a picture that could not be
read from the client, the next frame must be a key frame. The driver logs
nothing for a client's mistake: a hostile client must not be able to fill
the kernel's log.

## Power and lifetime

Opening the device turns the engine's clocks on and releases its reset;
closing it asserts the reset, turns the clocks off and frees the buffers,
in that order. The product's encoder runs only while somebody watches, so
the engine is powered only then. The module cannot be unbound or unloaded
while the device is open.

The driver binds to the engine's node in the device tree, which `cedrus`
also wants. A board that uses this driver does not load `cedrus`
(V6.1): the product does not decode with the engine.

## Licence

The module is GPL-2.0-only, like the kernel, a work of its own beside the
application (`docs/licensing.md`). Its register plan is written from this
project's own description of the engine, which comes from open sources
and from traces of the vendor's library; it contains no vendor code.

## How it reaches a board

The update-runtime package carries the module built for the board's
kernel release and for no other, loads it at boot, and gives its device
node to the encoder unit's group (V6.1). The lab module and the lab tool
are never packaged.

## Tests

- On the build host (`kernel/hardened-blikvm-ve/test`): the request checks
  and the plan as plain functions; 200000 random requests, in streams of
  up to forty passes, with the invariants above. The test keeps a second
  account of the interface in the interface's own words (every pass has a
  number, every set the number of the pass that wrote it) and requires of
  every pass that it reads the pass its picture refers to and writes over
  neither that one nor, in a second pass, the one before it. The plan's
  operations for the lab's sizes, both searches and all three sets are
  equal to those of the lab tool's hypothesis `h6`, register for register.
- On the board: a hostile client (`tools/device/ve_enc_hostile.py`) sends
  every request out of range, out of order, twice, with memory it does not
  own, pictures of noise at QP 0 that do not fit the stream buffer, under
  a rain of signals, closes in the middle and dies in the middle, sends 20000 random requests, and then opens, configures, uses
  and closes a thousand streams of the largest size. Afterwards the
  kernel's log has no new line but the test's own notes that it dropped
  the page cache, and of the board's contiguous memory
  nothing is left taken: the kernel does not say who holds that memory and
  lends what is free to its page cache, so the test drops the page cache
  before each count, and the measure is that no more than 16 MiB less is
  free than before, where the smallest buffer of such a stream left behind
  a thousand times would be 20 MiB.
- Gate V4: the lab tool's sequences through the driver give, byte for
  byte, the streams the lab module gave for the same pictures and QPs.

## Record

### Gate V4, 2026-10-03: passed

**The same streams.** Through the driver the lab tool's sequences give,
byte for byte, the streams the lab module gave for the same pictures, QPs
and rate control (`tools/device/ve_driver_compare.sh`; lab tool `b0cd482`,
the search chosen for every frame, at most two passes a frame):

| Size | Frames, a key frame every | Rates | Sequences | Streams the same |
| --- | --- | --- | --- | --- |
| 320x240 | 30, 30 | QP 30 | drag | 1 of 1 |
| 1920x1080 | 300, 300 | QP 30, 2.5 and 8 Mbit/s | scroll, drag, motion, pointer | 12 of 12 |
| 1366x768 | 300, 300 | QP 30, 2.5 Mbit/s | scroll, drag, motion, pointer | 8 of 8 |
| 1280x720 | 300, 60 | 1.5 Mbit/s | scroll, motion | 2 of 2 |

The engine takes the same time either way, 7.0 ms a pass at 1920x1080;
the loop around it takes about 2 ms more through the driver, for the
picture's copy in and the frame's copy out. The first stream through the
driver, 30 frames of `drag` at 320x240, decodes in the board's ffmpeg
without a complaint.

**V4.3.** On the build host: 200,000 random requests in 9,251 streams
(146,276 planned and checked, 1,400 sizes and 43,073 frames refused, 2,909
failures), every invariant held, and the driver's operations the lab
tool's for 14 passes. On the board, the hostile client: every request out of range, out of order, twice, at memory that is not the client's (none, address 0, the kernel's, memory that cannot be written), 425 of them, each answered as the interface promises; pictures of noise at QP 0: at 1920x1088 and 1366x768 the key frame is larger than the stream buffer and ends with `EIO` and the stall status 2, and the next key frame is made, at 1024x768 and 256x64 it fits (2,037,592 and 42,553 bytes); 200 encodes at 1920x1080 under 210 signals, every one finished; 20 clients killed in the middle of a frame, and the device opened again after each; 20,000 random requests (13,920 `EINVAL`, 3,222 `EFAULT`, 1,948 `ENOTTY`, 910 frames); a thousand streams of 1920x1088 opened, configured, used for a frame and closed. Afterwards no line in the kernel's log but the client's own note that it dropped the page cache, and 126,264 kB of contiguous memory free, as before.

**The board as it was.** Every run loaded the driver or the lab module in
`cedrus`'s place and gave the engine back: the module unloaded, `cedrus`
bound and suspended, the kernel's image compared word by word and
unchanged (50 runs), no module, file or process left. The driver wrote
no line into the kernel's log: the sessions' lines are `cedrus`
registering its device again after each run, the lab module's line when
it is bound, and the hostile client's notes that it dropped the page
cache. The memory scan found the parameter sets of the 320x240 stream
twice, in the two stream files the comparison keeps on tmpfs while it
compares them, as it finds every stream file a session leaves there.

The hostile client needed three runs, for faults of its own. The first
ended after five requests: Python cannot pass an address of the process
as a number to `ioctl`, and the client counted the kernel's notes of its
own page-cache drops as forbidden lines. The second ran through and found
every request "at no memory" answered with a frame: its hole was a mapping
given back, and its own 1 MiB stream buffer had been mapped at the same
place right after; the driver had read the client's memory, as it should.
The hole is now a mapping without access, kept for the whole test. In
both runs everything else held as in the third. And a first session of the
first stream never ran its command: a session takes its command as words,
not as one line of a shell.

Receipts: `evidence/private/ve-lab-v4-20261003-first`, `-compare`,
`-compare-b`, `-compare-c`, `-hostile`; and `-first-argv`,
`-hostile-test-bug`, `-hostile-hole-reused` for the tries above.

## What changed against the plan

| The plan's design | This note | Why |
| --- | --- | --- |
| The module is called `hardened-blikvm-ve` | `hardened-blikvm-ve-enc`, beside the lab's `hardened-blikvm-ve-lab`, both in `kernel/hardened-blikvm-ve/` | two modules are built from the shared part, and each name says which it is |
| The input picture and the bitstream are mapped into the client | no mapping; the driver copies the picture in and the frame out | the lab measured memory that is mapped uncached into a process: read at 60 MB/s. Copies through cached pages that are handed to the device and back cost 3 ms for a 1920x1080 picture, and a client without a mapping has nothing whose lifetime the driver must police |
| Three requests: configure, encode, read the result | the result comes back with the encode request | one request a frame is one sleep and one place where a frame can fail |
| A frame is encoded, key or not, at a QP | a frame may be made in several passes, with either of two searches, and the client says which pass is the frame | V3 found that neither of the engine's searches is right for every picture and that the engine is fast enough to try both (`h616-ve-h264-encoder.md`, V3.4 and V3.5); the choice is the rate control's, which is the client's by the plan |
| A device of its own or a V4L2 encoder, to be decided | a device of its own | a V4L2 stateful encoder would bring the framework's buffer queues and controls into a module whose whole point is to be small enough to check; the product has one client that needs none of it |

## Not in this version

Decoding; JPEG; B frames; pictures wider than 1920; the engine's own rate
control and regions of interest; more than one client; any other kernel
than the board's.
