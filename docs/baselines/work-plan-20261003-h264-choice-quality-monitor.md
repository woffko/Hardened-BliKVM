# Work plan: H.264 by engine in the video menu, and a quality monitor (2026-10-03)

Since the hardware H.264 plan of 2026-10-02 the test board encodes H.264
with its video engine, and with x264 where the engine is not there or
fails (`docs/baselines/audit-20261003-hardware-h264.md`). The operator
asked for two things on top of it:

1. The video menu offers H.264 by the hardware engine and H.264 by x264 as
   two entries, so that either can be chosen.
2. A quality monitor like RustAdmin's: a button on the toolbar ("QM") that
   opens a small window over the picture with what the picture costs and
   how it arrives, and, in its extended mode, much more.

Rules as in the earlier plans: capability-gated surfaces enforced by the
server and the board owner, English repository text, secrets only through
`project-memory-exec` (never in arguments, logs or receipts, and never a
program on its standard input under it), every change shipped as a signed
slot or through the encoder's deploy script and verified on the test board,
audit and design documents updated with evidence under `evidence/private`.
Additions for this plan:

- One commit per item. An item's own tests run before its commit; the full
  `scripts/check.sh` runs before the last commit of each phase and before
  every slot or package build. The branch is pushed after each phase.
- Phases end in gates with numbers. A gate that is missed, or a time box
  that runs out, ends the plan there with the findings recorded; what was
  finished stays, behind its switch or unused.
- Nothing here takes the HID lease, sends input to the target or pulses ATX.
  The probes watch; they do not touch the target.
- The monitor reads and shows; it changes nothing on the board. Its reads
  run only while it is open.

## Where the work starts

- Slot 66 `0.1.0-alpha.11` (source `b31149a`), the update runtime
  `0.1.0~dev36` with the encoder driver switched on, and the H.264 encoder
  of `b31149a` on the test board.
- One H.264 encoder process serves every H.264 viewer: WebRTC through
  Janus, and QUIC through its frame socket. It opens the engine when a
  viewer's stream starts and uses x264 when the engine is not there or
  fails three frames in a row. Which of the two encodes is one word in
  `/run/hardened-blikvm-h264/encoder`, which `GET /api/vm/screen` reports
  as `h264Encoder`.
- The web server holds a demand connection to the encoder for every
  viewer and sends it the codec and the WebRTC quality (`kbps N` lines); the
  quality is the board's, stored in the server's state directory and
  changed through `POST /api/vm/screen` (`webrtc_kbps`).
- The video menu (`web/src/pages/desktop/menu/screen/video-mode.tsx`) has
  four entries: H.264 (Direct), H.264 (WebRTC, or QUIC where this browser
  uses the QUIC session), VP8 (WebRTC) and MJPEG. The choice is the
  browser's own (local storage).
- What the page knows of its picture today: the QUIC session counts frames,
  their age at drawing by the board's clock, frames given up, key-frame
  requests and the gateway's counters; the WebRTC view counts nothing
  (`RTCPeerConnection.getStats()` is not read); the MJPEG view is an image
  element and counts nothing. The encoder writes a statistics line every
  ten seconds into its journal: frames a second, the stages' times, the
  engine, its passes, frames left out and failures.

## Reference: RustAdmin's quality monitor

The reference is woffko/rustadmin, branch `experimental`
(`flutter/lib/common/widgets/overlay.dart`, `QualityMonitor`), itself a
fork of RustDesk (AGPL-3.0). What it is:

- A toggle in the session's toolbar shows or hides it; it sits over the
  picture, translucent, in a corner.
- Basic: Speed, Delay (green), Path, and under "QoS" FPS, Bitrate, Codec
  and Chroma.
- Extended (chosen in the monitor's own header: Basic or Extended): a
  window that can be dragged by its header, resized at its corner and
  scrolled, opaque or translucent, with sections. The transport: the QUIC
  application and video transport, queue target, path MTU, datagram sizes,
  RTT, lost packets, reassembly drops and their reasons, frames received,
  key-frame requests and their kinds, receiver and sender recovery, frames
  sent and refused, percentiles of the sender's delays, and drops; QoS as
  above; "Remote" (the host's version, capture API and frame, encoder API
  and input, video threads, frame size, FPS mode, profile, pacing, the host
  pipeline's 95th percentile, delivery phase, recoveries, the last stall);
  "Local" (the client's version, decoder, renderer, decode FPS, display
  refresh, queue, frames received, decoded and shown, dropped, decode and
  submit times).

The operator allows taking its code. It is Dart in Flutter on top of a Rust
client, and this page is TypeScript in React on top of the browser's own
decoders, so the monitor is written anew after it: its layout, its two
levels, its rows where this product has the same thing, and rows of this
product's own where it has others. Were any of its code taken, it would be
RustDesk's under AGPL-3.0: `docs/licensing.md` records it, and the page's
licence statement says so.

## Design

### H.264 by engine

The engine stays a property of the board, not of a browser: one encoder
serves every H.264 viewer, and two encoders for two choices would decode
the capture twice. So:

- The board has an **H.264 engine choice**, `hardware` or `software`
  (x264), stored like the WebRTC quality in the server's state directory,
  `hardware` by default where the engine is switched on.
- `POST /api/vm/screen` with `h264_engine` sets it, under the same right as
  the WebRTC quality. `GET /api/vm/screen` reports the choice
  (`h264Engine`) and, as today, what encodes (`h264Encoder`), and whether
  the board has an engine to choose (`h264HardwareAvailable`, from the
  encoder's own report).
- The server sends the choice on every demand connection after the codec,
  as it sends the quality (`engine hardware` or `engine software`), and
  again when it changes.
- The encoder takes the choice at its next frame. `software` while the
  engine encodes: x264 takes the stream over where it is, as after
  failures, but it counts as no failure. `hardware` while x264 encodes: the
  engine is opened and tried with that frame as a key frame; if it cannot
  be had, x264 goes on and the encoder says why. The status word follows.
- The video menu shows two H.264 entries where it showed one: **H.264 HW**
  and **H.264 SW**, each with the transport in brackets as today (WebRTC or
  QUIC). Choosing one sets this browser's mode to H.264 and the board's
  engine choice. The check mark follows the board's choice; an entry whose
  engine is not the one that encodes (a choice of `hardware` with x264
  encoding) says so beside it. Without an engine on the board, H.264 HW is
  shown disabled with the reason; without the right to change the choice,
  both entries choose this browser's mode only and say who sets the
  engine.
- Changing the engine changes it for every H.264 viewer at once; the
  menu's tooltip says so.

### The quality monitor

- **QM** on the toolbar, beside the screen menu, in the desktop and the
  mobile layout, hideable like the other toolbar items. It opens and closes
  the monitor; open or closed and its level are remembered by the browser.
- **The window**: over the picture in the top left corner, translucent,
  with a header that holds the title, the level (Basic or Extended) and a
  close button, and by which it is dragged; in Extended it can be resized
  at its corner, scrolls, and can be made opaque. On a phone it is
  narrower and its text smaller, as in the reference.
- **Basic**: Speed (what arrives, Mbit/s), Delay (a frame's age at drawing:
  measured by the board's clock over QUIC, estimated over WebRTC, not known
  for MJPEG), Path (MJPEG, H.264 Direct, WebRTC or QUIC), and under QoS:
  FPS (drawn, and the capture's), Bitrate (the target), Codec (H.264 HW,
  H.264 SW, VP8, MJPEG, with the picture's size) and Chroma.
- **Extended**, in sections like the reference's:
  - *Transport*: over QUIC the session's RTT, lost datagrams, frames
    received, given up and requested again, and the gateway's frames sent,
    dropped and key frames; over WebRTC the candidate pair's RTT, packets
    received and lost, jitter, NACK, PLI and FIR counts, the jitter
    buffer's delay; over MJPEG what the server's relay counts.
  - *Board*: the slot's version; the capture's size, rate and JPEG
    quality; the encoder's engine (the video engine, x264 or libvpx), its
    frames a second, the stages' times (JPEG decoding, conversion,
    encoding), the engine's time a pass and passes a frame, frames left
    out, key frames and failures, the gate's frames let through and kept
    back; the encoder unit's share of the processors; the SoC's
    temperature.
  - *Local*: the browser's decoder (WebCodecs or the browser's WebRTC
    stack, hardware or not where it says), frames received, decoded,
    drawn and dropped, decode time, the display's refresh.
  - Rows that do not apply to the path in use are left out; values that are
    not known show "-".
- **The board's side** comes from one read-only API, `GET /api/vm/quality`,
  served from a cache of at most a second. The encoders (H.264 and VP8 run
  the same program) write a small JSON report into their runtime
  directories once a second while they encode (bounded in size, replaced
  whole, readable for the web server like the status word); the server
  reads it with the capture's numbers it already has, the encoder unit's
  processor time from its control group, and the temperature the broker
  already observes. The page asks once a second while the monitor is open,
  and never otherwise.
- **The page's side** comes from what each view already has or can read
  without cost: the QUIC session's counters, `getStats()` once a second for
  WebRTC, the Direct view's decoder, and for MJPEG what the server counts
  (the image element counts nothing).

## Phases

### C0: preparation (nothing on the board is touched)

- C0.1 The design note `docs/design/h264-engine-choice-and-quality-monitor-v1.md`:
  the design above, the API's fields with their units and bounds, the
  encoder's report, the reference and what is taken from it, and the
  licence question.
- C0.2 The fixtures for the page's contracts: a stand-in for the new API
  and the encoder's report, so that the page can be tested on the host.

### C1: the engine as the board's choice (at most 8 hours; gate)

- C1.1 The encoder: the `engine` line on the demand socket; switching at
  the next frame both ways, without counting a failure; the status word;
  the report file (C3 fills it).
- C1.2 The server: the choice, stored and restored, under the WebRTC
  quality's right; sent on every demand connection and on change;
  `h264Engine` and `h264HardwareAvailable` in `GET /api/vm/screen`.
- C1.3 Tests on the host: the encoder against the engine's stand-in
  switching both ways under a stream (the frame socket's records go on,
  a key frame follows every switch); the server's choice, its storage and
  its demand lines.
- **Gate C1**: on the board the engine is switched 50 times each way while
  the picture moves and while a viewer watches: in a transient instance
  fed with the lab's pictures as in V7.1, whose demand is held on the board
  and switched by its `engine` lines; and in the product's own encoder
  through `POST /api/vm/screen` while a device probe watches. Every switch
  takes effect within 2 s, the status word and `h264Encoder` follow it, the
  stream goes on with a key frame, there is no encoder failure and no
  kernel line, and the processor time follows the engine.

### C2: the menu (at most 4 hours)

- C2.1 H.264 HW and H.264 SW in the video menu, with the transport in
  brackets, the check mark on the board's choice, the disabled and the
  read-only cases and their tooltips, English and Russian.
- C2.2 The page's contract against the stand-in: both entries, a choice
  that is not what encodes, no engine on the board, no right to change it.

### C3: the board's side of the monitor (at most 6 hours)

- C3.1 The encoders' report: once a second while encoding, bounded, the
  numbers of the statistics line and the gate's counts; the VP8 unit too.
- C3.2 `GET /api/vm/quality`: the encoders' reports, the capture, the
  encoder unit's processor time, the temperature; a cache of a second;
  every field checked and bounded; nothing when the reports are absent.
- C3.3 Tests on the host: the report's writer and reader with damaged and
  oversized files, the API's fields, its cost.

### C4: the monitor (at most 10 hours; gate)

- C4.1 The QM button, the window (drag, resize, scroll, opaque, close),
  the two levels, remembered by the browser; desktop and mobile.
- C4.2 The page's own numbers for each path: QUIC from the session,
  WebRTC from `getStats()`, Direct from its decoder, MJPEG from the server.
- C4.3 The rows of Basic and Extended as above, English and Russian.
- C4.4 Contracts on the host: the window and its levels; the rows of each
  path against the stand-in; the API asked only while the window is open.
- **Gate C4**: on the host against the real server, gateway and encoder
  stand-in, the monitor's numbers match what the contracts count (frames a
  second within 10%, the bytes' rate within 10%, QUIC delay within 10 ms of
  the probe's median); the page's own cost of the open monitor is under
  2 ms a second of the main thread.

### C5: the board, the documents (at most 8 hours; gate)

- C5.1 Slot and encoder on the board (`deploy_vp8.sh` with the new
  encoder; the update runtime unchanged unless an item needs it).
- C5.2 On the board, both engines and every path the board offers: the
  monitor against the device probes (`quic_video_device_probe.mjs`,
  `vp8_webrtc_probe.mjs` with H.264 and VP8, the MJPEG latency probe), the
  engine switched from the menu, the monitor's cost on the board.
- C5.3 Regression: GUI walks with the monitor open and closed, VP8,
  MJPEG, the firewall probe; a slot from before this plan with the new
  encoder still shows H.264.
- C5.4 Documents: the design note, the video design note, an audit note
  with every item and its evidence, `docs/licensing.md` if anything of the
  reference was taken; the memory note.
- **Gate C5**: on the board the menu's choice changes the engine within
  2 s for a watching viewer, both ways, and the monitor shows which one
  encodes; the monitor's Basic numbers agree with the probes as in gate
  C4; with the monitor open the web server's and the encoder's processor
  time grow by less than 2% of a core; no API error and no kernel line.
  Missed: the menu keeps the one H.264 entry and the monitor stays behind
  its own switch, off, with the numbers that say why.
- C5.5 End state: every administration switch and the operator's settings
  as found, the engine choice as the operator left it (hardware unless the
  operator chose otherwise), no probe or file of the plan on the board.

## What the operator should expect

- The engine is the board's: choosing H.264 SW in one browser makes every
  H.264 viewer's picture come from x264 until somebody chooses HW again.
  x264 costs one and a half cores more for a moving picture and makes
  fewer frames of it (`docs/design/h616-ve-h264-encoder.md`, "V7").
- The monitor shows what the board and the browser know. Over WebRTC a
  frame's age is an estimate; over MJPEG the browser counts nothing.
- With the monitor open the page asks the board once a second.

## Out of scope, possible follow-ups

- An engine per viewer (two encoders).
- The encoder's gate, which lets about twenty frames a second of a
  60-frame capture through for a limit of 30 (finding 7 of the hardware
  plan's audit): the monitor will show its counts; changing it is its own
  plan.
- The QUIC gateway's certificate made before the clock is set, and the
  firewall status that may be five seconds old after a change.
- Recording or exporting the monitor's numbers.

## Risks

- A switch from x264 to the engine in the middle of a stream is new: the
  engine's stream begins with a key frame, and a decoder must take it
  without a reset. Gate C1 shows it on the board.
- `getStats()` costs the browser something; once a second it is small,
  and gate C4 measures it.
- A report file the server reads is a new input from another process: it is
  bounded and checked like the status word.

## Operator

- Decides, by the goal's text, whether the slot and the encoder may be
  installed on the test board (C5).
