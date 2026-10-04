# H.264 by engine in the video menu, and a quality monitor

Plan: `docs/baselines/work-plan-20261003-h264-choice-quality-monitor.md`.
This note fixes what the plan leaves to its design: the API's fields with
their units and bounds, the encoder's report, the demand line, the menu's
cases, the monitor's rows and where each comes from, the reference and the
licence question. The record of what was measured follows at its end as the
work goes on.

Status (2026-10-04): done. The engine as the board's choice, the menu,
the board's side of the monitor and the monitor are built; gates C1 (on
the board), C4 (on the host) and C5 (on the board) passed; the test board
runs slot 72 `0.1.0-alpha.16` with the engine chosen, its settings as they
were found. The plan's outcome item by item is
`docs/baselines/audit-20261004-h264-choice-quality-monitor.md`.

## The engine as the board's choice

One H.264 encoder process serves every H.264 viewer of the board, over
WebRTC through Janus and over QUIC through its frame socket. The engine it
encodes with is therefore the board's, not a browser's.

### The choice and where it lives

- The choice is `hardware` (the video engine, through
  `/dev/hardened-blikvm-ve-enc`) or `software` (x264). It is `hardware`
  until somebody chooses otherwise; where the board has no engine the
  encoder uses x264 whatever the choice says.
- The web server stores it in its state directory as `h264-engine.json`,
  `{"engine":"hardware"}` or `{"engine":"software"}`, at most 256 bytes,
  mode 0600, written to a temporary file and renamed, like the WebRTC
  quality (`webrtc-bitrate.json`). A file it cannot read or understand is
  the default.
- `POST /api/vm/screen` with `{"type":"h264_engine","value":1}` chooses
  `hardware`, with `"value":0` `software`: the body keeps its numeric value
  as for every other setting of that request. Any other value is refused
  with `value_out_of_range`; a server without the H.264 encoder's demand
  socket refuses with `unsupported_setting`. The answer is
  `{"applied":true,"h264Engine":"hardware"|"software"}`.
- The right is the WebRTC quality's: the request is served to an
  authenticated session where the video controls are enabled, and the
  capabilities offer the setting as `h264_engine` in `video.settings`
  where they offer `webrtc_bitrate`, the H.264 demand socket is configured
  and the H.264 encoder's report is configured (changed in the review
  fixes, Q3 and Q7: the report's path is installed with the encoder that
  writes it, so it says that an encoder that knows the choice is there, and
  a slot on a board with an older encoder offers nothing that would change
  nothing). What is not offered is not served either: `POST /api/vm/screen`
  with `h264_engine` or `webrtc_kbps` answers `unsupported_setting`, and
  `GET /api/vm/screen` has no `h264Engine`. Before, the video controls hid
  the entries in the page only, and the server served both settings to any
  session with a CSRF token.
- `GET /api/vm/screen` reports, beside today's fields:
  - `h264Engine`: the choice, where the setting is offered;
  - `h264HardwareAvailable`: `true` or `false` from the encoder's report,
    absent while the encoder has not reported; `false` also where the
    encoder's unit names no device (the report's `null`);
  - `h264HardwareReason`: the encoder's reason where that is `false` and
    it gives one, at most 120 characters (added in C1.2: the menu's
    tooltip shows it, and the menu must not ask the quality API, which is
    asked only while the monitor is open);
  - `h264Encoder`: as today, `hardware` or `x264`, what encodes.

### On the demand connection

The server already writes the codec and `kbps N` on every demand
connection and a new `kbps` line on every change of the quality. To the
H.264 encoder it writes `engine hardware` or `engine software` after the
`kbps` line, and again whenever the choice is made, also where it is the
one already made. Lines stay below the encoder's bound of 32 bytes. A
longer line is skipped, up to 4 KiB in all; a client that sends more without a
newline loses its connection (review fixes, Q13). The VP8 encoder never gets
the line.

The encoder counts a line as the choice made, which lets it try again an
engine it could not have, except a connection's first line where that
repeats the choice it has: a new viewer is no choice.

### In the encoder

- It keeps the last `engine` line it was given on any demand connection;
  without one it keeps to `hardware`, as before this plan.
- It takes the choice between two frames, also while no frame comes: a
  switch waits at most half a second for the encoder's loop, and the line
  lets the next captured frame through as a key frame.
- **At its start**, where `BLI_KVM_ENCODER_HARDWARE` names a device, it
  opens the device, asks its `INFO` and closes it again: that answers
  `hardwareAvailable`, with the reason when it is `false` (no such device,
  refused, an interface it does not know). A stream that opens later tries
  the engine again only where it is wanted.
- **When a stream opens**: the engine where it is wanted and has not been
  given up since the choice was last made (below), tried with a picture as
  today, whatever the look at the start found; otherwise x264.
- **When the choice changes during a stream**, at the next frame:
  - `software` while the engine encodes: x264 takes the stream over where
    it is, behind the same packetizer and frame socket, beginning with a
    key frame. This is no failure and is not counted as one.
  - `hardware` while x264 encodes: the engine is opened and configured
    for the stream's size, and that frame is its first, a key frame. If
    that fails x264 goes on, `hardwareAvailable` turns `false` with the
    reason, and the engine is not tried again until the choice is made
    again or a new stream opens.
- An engine that fails on three frames in a row is given up for x264 as
  before this plan, `hardwareAvailable` turns `false` with the reason, and
  it is tried again only when the choice is made again: a new stream would
  only fail on it again.
- Where a choice comes and nothing is about to try the engine (x264 is
  chosen, or no stream is open), the encoder looks at the device again, so
  that `hardwareAvailable` tells whether the engine could be chosen.
- The status word (`BLI_KVM_ENCODER_STATUS`) follows what encodes, as
  today; the report (below) says what is wanted and whether the engine can
  be had.

### The video menu

Where the capabilities offer `h264_engine`, the one H.264 entry becomes
two, each with the transport in brackets as today:

- **H.264 HW (WebRTC)** or **(QUIC)**, and **H.264 SW (WebRTC)** or
  **(QUIC)**.
- Choosing one sets this browser's mode to H.264 (as the one entry did)
  and, where the choice is not already the board's, posts it. The page
  reloads as it does after every change of mode.
- The check mark follows the board's choice while this browser's mode is
  H.264. Where what encodes is not what was chosen (`hardware` chosen, x264
  encoding), the chosen entry says so beside its name.
- `h264HardwareAvailable: false` with no reason (the unit names no device:
  there is no engine to offer): H.264 HW is shown disabled. With a reason
  (busy, no device node, three frames in a row failed) it stays choosable and
  its tooltip gives the reason and says that choosing it asks the encoder to
  try again; choosing hardware while the board's choice is hardware and x264
  encodes posts the choice again, which is the retry (changed in the review
  fixes, Q6: the encoder tries an engine that failed only when the choice is
  made again, and a disabled entry made that impossible from the menu; the
  one way back was to choose H.264 SW, which makes the encoder look at the
  device again, and then H.264 HW).
- Without the right (the setting not offered for writing), both entries
  only choose this browser's mode, and their tooltip says that the board's
  operator sets the engine.
- The tooltip of both says that the engine is the board's: every H.264
  viewer gets it.

## The quality monitor

### The window

- **QM**, a button on the toolbar beside the screen menu, in the desktop and
  the mobile layout, opens and closes it. It can be hidden like the other
  toolbar items.
- It lies over the picture, in the top left corner at first, translucent.
  Its header holds the title, the level (**Basic** or **Extended**) and a
  close button, and it is dragged by its header. In Extended it can also be
  resized at its bottom right corner, scrolls, and can be made opaque. On a
  phone it is narrower and its text smaller.
- Open or closed, its level, place, size and opacity are this browser's
  (local storage); nothing of it goes to the board.
- It reads once a second while it is open, and never while it is closed.
- Settled in C4 for its gate: the rows are drawn on a canvas by the
  monitor's own worker. Once a second the page counts its own picture and
  sends that to the worker; the worker asks `GET /api/vm/quality`, makes the
  rows and draws them; the canvas is the compositor's, like the QUIC
  picture's. The page's main thread only counts and sends. Rows that React
  drew in the document cost the main thread 4 to 5 ms a second on the host
  with the QUIC picture (the request's work and the browser's frame for
  every change of text), more than the 2 the plan allows; the worker's
  canvas costs it 0.2 to 0.8. The header with its buttons stays in the
  document. The rows' text is not in the document: the canvas is an image
  named for the monitor, and the contracts read what was drawn from the
  worker.
- Limits, written down in the review fixes (Q1, Q12):
  - It needs workers and `OffscreenCanvas`. A browser that lacks them, or
    refuses the worker, gets one line in place of the rows (English and
    Russian) and nothing else: no rows, no timer, no request. There is no
    drawing fallback on the main thread: it would bring back the 4 to 5 ms a
    second that the worker removed, and a second renderer to keep right.
  - The rows are an image. A screen reader gets the window's name and the
    header's buttons and not the numbers, and nothing in the rows can be
    selected or copied. A text alternative is not built: text kept current in
    the document is what cost the main thread 4 to 5 ms a second.
  - The worker keeps five values on its scope for the contracts and the
    device probes, which read them and nothing else of it:
    `qualityMonitorRows`, `qualityMonitorLabels`, `qualityMonitorSections`,
    `qualityMonitorDraws` (a probe that reads more often than the monitor
    draws takes every draw once) and `qualityMonitorPixels` (the canvas's size
    in device pixels, which shows that the rows follow the display's pixel
    ratio).

### Basic

| Row | Shows | Where it comes from |
| --- | --- | --- |
| Speed | what arrives, Mbit/s, to a kilobit below one megabit | WebRTC: `bytesReceived` of the video's inbound RTP; QUIC: the frames' bytes the session brought; MJPEG: - |
| Delay | a frame's age when it is drawn, ms | QUIC: measured by the board's clock (the session already has it); WebRTC: estimated, the jitter buffer's delay a frame and half the round trip, marked `~`; MJPEG: - |
| Path | MJPEG, H.264 Direct, WebRTC or QUIC | the page's mode and transport |
| FPS | frames drawn a second / the capture's | the page's count; the board's capture rate |
| Bitrate | the target, kbit/s | the WebRTC quality (both encoders and QUIC); MJPEG: the JPEG quality instead |
| Codec | H.264 HW, H.264 SW, VP8 or MJPEG, and the picture's size | the encoder's report, the page's decoder |
| Chroma | 4:2:0 for H.264 and VP8, 4:2:2 for the capture's JPEG | the codec |

Refined in C5.2: the Speed has three places below one megabit a second
instead of two. On the board's still screen a second brings 0 to 40
kilobits, and with two places VP8's small frames of about five kilobits
were shown as ten: the monitor's Speed was 18% above what `getStats()`
counted over the same two minutes, every byte counted right.

### Extended, in sections

**Transport** (rows of the path in use only):

- QUIC: round trip, datagrams lost, frames received, given up and asked
  for again (key-frame requests), and the gateway's frames sent, dropped
  and key frames, all from the session's own counters.
- WebRTC, from `getStats()`: the selected candidate pair's round trip and
  its type, packets received and lost, jitter, the jitter buffer's delay a
  frame, NACK, PLI and FIR counts, frames received, decoded and dropped.
- MJPEG: the capture's rate and the rate the server queues.

**Board**:

- The slot's version.
- The capture: size, frames a second, the rate queued, the JPEG quality.
- The encoder that serves the path (H.264 or VP8): its engine (the video
  engine, x264 or libvpx) and the one chosen; frames a second; the stages'
  times a frame (JPEG decoding, conversion, encoding); for the engine its
  time a pass, passes and both-search passes a frame, frames at a higher
  QP, frames left out, key frames and failures; the gate's frames let
  through, kept back as too soon, and skipped as unchanged; the encoder
  unit's share of the processors.
- The SoC's temperature.

**Local**:

- The decoder: the browser's WebRTC stack (and its own name for its
  decoder where it gives one) or WebCodecs with its hardware preference.
- Frames received, decoded, drawn and dropped a second; decode time a
  frame; the display's refresh rate (measured with animation frames).

Rows that do not apply to the path are left out; a value that is not known
shows `-`; a value older than three seconds is shown as not known.

## The board's side

### The encoder's report

Both encoder units (H.264 and VP8 run the same program) write a report
where `BLI_KVM_ENCODER_REPORT` names a file: `/run/hardened-blikvm-h264/report.json`
and `/run/hardened-blikvm-vp8/report.json`. It is replaced whole (a
temporary file and a rename), mode 0644 so that the web server, another
user, can read it, at most 4096 bytes; once a second while a stream is
open and once when it closes or the encoder starts.

```json
{
  "schema": 1,
  "codec": "h264",
  "streaming": true,
  "engine": "hardware",
  "engineWanted": "hardware",
  "hardwareAvailable": true,
  "hardwareReason": null,
  "width": 1920,
  "height": 1080,
  "kbps": 8000,
  "maxFps": 30,
  "converters": 3,
  "encodedFrames": 123456,
  "stageMicros": { "decode": 28100, "convert": 9600, "encode": 17600 },
  "gate": { "admitted": 123500, "tooSoon": 61000, "unchanged": 5000 },
  "hardware": {
    "passes": 150000, "bothSearches": 30000, "wide": 20000,
    "higherQp": 900, "leftOut": 300, "keyFrames": 4100,
    "failures": 0, "engineMicros": 1100000000
  }
}
```

- `codec`: `h264` or `vp8`. `engine`: `hardware`, `x264`, `libvpx`, or
  `null` with no stream. `engineWanted`: `hardware`, `software`, or `null`
  for VP8. `hardwareAvailable`: `true`, `false`, or `null` where the unit
  names no device. `hardwareReason`: at most 120 characters or `null`.
- `width`, `height`: the stream's, 0 to 8192. `kbps`: 64 to 20000.
  `maxFps`: 1 to 60. `converters`: 1 to 4.
- Counters (`encodedFrames`, `gate.*`, `hardware.*`) count since the
  encoder's start, so that a reader makes rates from two reports;
  `engineMicros` is the engine's time summed over its passes.
- `stageMicros`: the mean a frame over the last second, 0 to 10,000,000.
- `hardware` is `null` where the engine has not encoded since the start.
- Schema 1 is closed. The server reads the report with every field checked
  and refuses a whole report that has a field it does not know, so a field
  added to schema 1 would make an older server show no board numbers at all.
  A new field needs a new schema number and a server that reads it.

### `GET /api/vm/quality`

Read-only, for an authenticated session; the server builds it at most once
a second and serves the same answer within that second.

```json
{
  "capture": { "width": 1920, "height": 1080, "fps": 30.0, "queuedFps": 30.0, "quality": 100 },
  "h264": { "...": "the encoder's report, checked, with rates" },
  "vp8": null,
  "units": { "h264CpuPercent": 156.0, "vp8CpuPercent": null },
  "temperatureMillidegrees": 68300,
  "version": "0.1.0-alpha.12"
}
```

- `capture`: what `GET /api/vm/screen` already reads from the engine's
  state.
- `h264`, `vp8`: the report where it is no older than three seconds and
  passes every bound above, with rates made from the last two reports:
  `fps` (frames encoded a second), `gatePerSecond` (`admitted`, `tooSoon`
  and `unchanged` a second) and, in `hardware`, `passesPerSecond` and
  `engineMicrosPerPass`; a rate is `null` until there are two reports.
  `null` otherwise. A report that is too large, not JSON, or out of bounds
  is `null`, and the server says so once in its log.
- `units`: each encoder unit's share of the processors over the last
  interval between two reads, in percent of one core, from its control
  group's `cpu.stat`; `null` where it cannot be read.
- `temperatureMillidegrees`: what `/api/inventory` already reads, or
  `null`.
- `version`: the slot's, as the update status names the selected slot, or
  `null` where the server has no update status. Refined in C5.3: the
  update runtime's status takes about half a second to answer, and an
  answer that waited for it took 0.5 to 0.6 s on the board; when the
  settings' System tab asked the board for more at the same time, it
  took longer than the monitor's 0.9 s and the monitor gave it up. The
  version changes with an update only: it is read in the background at
  most once a minute, and an answer has the version read last (`null`
  before the first read has answered).

Settled in C3.2: a report's age and the span of its rates are the times its
file was written, so that a reader that comes twice within a second sees
the same pair of reports; the capture's JPEG quality is the owner's, asked
like `GET /api/vm/screen` asks it, once with every answer; a server built
without the owner's surface answers the capture with `null` fields.

Refined in the review fixes (Q5): no rate is made across a pause. A report
that is absent, refused or older than three seconds ends the stored pair, and
two reports are paired only when they were written no more than three seconds
apart; a unit's processor time is compared only with a reading no older than
ten seconds. Before, the first answer after the monitor had been closed made
its rates over the whole pause (a number near zero for a second, where a dash
belongs).

## The page's side

- **QUIC**: the session's own counters, which `useQuicVideo` and the
  receiver already keep: frames, their age at drawing, given up, key-frame
  requests, round trip, lost datagrams, the gateway's counters.
- **WebRTC**: `RTCPeerConnection.getStats()` once a second while the
  monitor is open: the video's `inbound-rtp`, its `codec`, the selected
  `candidate-pair`.
- **H.264 Direct**: the bytes it is sent, where the board offers the mode;
  its decoder counts nothing the monitor could read (settled in C4.2).
- **MJPEG**: the image element counts nothing; the monitor shows the
  board's capture and queue only.

## Reference and licence

RustAdmin's quality monitor (woffko/rustadmin, branch `experimental`,
`flutter/lib/common/widgets/overlay.dart`, `QualityMonitor` and its model)
is the reference: its toggle on the toolbar, its translucent window in a
corner, its Basic rows (Speed, Delay, Path; under QoS FPS, Bitrate, Codec,
Chroma), its Extended level chosen in its own header, with a window that is
dragged, resized and scrolled and may be opaque, and its sections for the
transport, QoS, the remote side and the local side. The operator allows
taking its code. It is Dart in Flutter over a Rust client; this page is
TypeScript in React over the browser's decoders. The monitor is written
anew after it: its layout, its two levels and the meaning of its rows where
this product has the same thing. Were any of its code taken, it would be
RustDesk's under AGPL-3.0: `docs/licensing.md` would record it, and the
page's licence statement would name it.

## Tests

- **Encoder** (host): the `engine` line parsed and bounded; switching both
  ways under a stream against the engine's stand-in, with the frame
  socket's records going on and a key frame after every switch, no failure
  counted; a choice of `hardware` without a device stays on x264 with the
  reason; the report's fields, bounds and size.
- **Server** (host): the choice stored, restored and refused as above; the
  demand lines on connection and on change; the screen's new fields; the
  report's reader with damaged, oversized, stale and out-of-bound files;
  `GET /api/vm/quality` and its cache.
- **Page** (host, Playwright): the menu's cases against a stand-in
  (`tools/playwright/quality_fixture.mjs`): both entries, the check mark,
  a choice that is not what encodes, an engine that failed or is busy (shown
  with its reason, choosable again), no engine, a refused choice, no right;
  the monitor's window, its two levels, its rows for each path against the
  stand-in, the API asked only while it is open, a browser without
  `OffscreenCanvas` or with the worker refused, the rows drawn again when the
  pixel ratio changes.
- **Page** (host, `node --test`): `tools/test_quality_monitor.mjs` holds the
  monitor's pure functions (rates, the rows of each path, the stored window)
  without a browser.
- **Under load** (host): `scripts/stress-under-load.sh RUNS [LOAD] --
  COMMAND...` runs a command again and again beside busy loops, one and a
  half times the cores by default, and counts the runs that passed. The tests
  of this plan that depend on time are held to 30 runs under it without a
  failure (gates C and E of the review fixes; the numbers are in the Record).
  `check.sh` runs only the tool's own unit test, `tools/test_stress_under_load.py`:
  a run takes minutes.

## Record

### C1, 2026-10-03: the engine as the board's choice

Built: the encoder's `engine` line, its switch between two frames both
ways, its look at the device and its report (C1.1, 047f5f4); the server's
choice, stored and sent on the demand connections, and the screen's new
fields (C1.2, 7ec1c57); the host tests (C1.3, 6088fb9). On the test board:
slot 67 (`0.1.0-alpha.12`, source 6088fb9) and the encoder of the same
source through `deploy_vp8.sh`; the update runtime (dev36) and the driver
as they were.

**Found on the board: the choice reaches the encoder only on a demand
connection.** While nobody watches H.264 the server stores a choice and
answers it, and the encoder takes it from the next viewer's connection,
whose first lines name it: until then `h264Encoder` names the engine that
encoded last while `h264Engine` already names the new choice. A browser
that chooses an H.264 entry watches H.264 right after it (the entry sets
its mode), so the menu meets this only where it shows what encodes to a
browser in another mode: it says so only while this browser's mode is
H.264 (C2).

**Gate C1, a transient instance** (`tools/device/ve_engine_switch_run.sh`
in a lab session): the product's binary with the unit's settings, fed with
the lab's `drag` pictures at 60 a second into its limit of 30 at 8 Mbit/s,
its demand held on the board and switched by `engine` lines
(`ve_engine_switch.py`), 100 times, four seconds apart, while it encoded.
The product's own encoder was given x264 for the session and the engine
after it, through its demand socket as a viewer's connection gives it
(`ve_engine_choose.py`): its status word followed after 0.08 and 0.13 s.

| | To x264 | To the engine |
| --- | --- | --- |
| Status word followed the line, s (median, highest) | 0.074, 0.094 | 0.107, 0.149 |
| First key frame after the line, s (median, highest) | 0.057, 0.116 | 0.135, 0.177 |
| Frames before that key frame (at most) | 1 | 1 |
| Frames a second (median of the periods) | 17.0 | 20.3 |
| Cores (median of the periods) | 2.87 | 1.13 |

7,659 records came over the frame socket, one stream, every one the next;
241 key frames. The journal has 51 lines that the engine encodes (the first
stream's and 50 switches), 50 that x264 is chosen, and none of a failure;
the kernel said nothing. The frame before the engine's first is the one x264
was encoding when the line came.

**Gate C1, the product's own encoder** (`tools/device/engine_switch_probe.py`,
`ve_engine_watch.py` on the board): 100 choices through `POST /api/vm/screen`,
five seconds apart, while `quic_video_device_probe.mjs` watched for 580 s.
The target showed a moving picture (17.2 frames a second at the viewer).

| | To x264 | To the engine |
| --- | --- | --- |
| `h264Encoder` followed the request, s (median, 95 in 100, highest) | 0.134, 0.190, 0.234 | 0.209, 0.318, 1.266 |
| The unit's share of the processors while it encoded, % of a core | 244 | 136 |

The request was answered in 28 ms in the median (72 at most). The board's
watcher saw the status word change 101 times; the unit ran one process
throughout and held at most 48 MB. The viewer decoded 9,973 frames: none
given up, no key-frame request, no restart of its decoder; a frame was 78 ms
old in the median when it was drawn (200 in 95 of 100). The gateway sent
10,240 frames, dropped 2, sent 350 key frames and lost no packet. The
journal has 51 and 50 lines as above and none of a failure; the kernel said
nothing.

**Gate C1 passed**: both ways 50 times in each setting, every switch within
1.3 s (2 s allowed), the status word and `h264Encoder` with it, the stream
on with a key frame, no encoder failure, no kernel line, and the processor
time with the engine. The choice was left as found (`hardware`), and with it
the board's other settings.

Receipts: `evidence/private/c1-transient-gate`, `c1-product-gate`,
`c1-product-gate-switches.json`, `c1-product-gate-quic.json`,
`update-to-seq67-alpha12-*`, `c1-state-as-found-20261003.json`,
`c1-state-after-gate-20261003.json`.

### C2 to C4, 2026-10-03: the menu, the board's side, the monitor

Built: H.264 HW and SW in the video menu (C2.1, 2348bf5) and its contract
(C2.2, 7e9e6ea); the encoders' report once a second with its numbers
(C3.1, 9e89e61), `GET /api/vm/quality` (C3.2, 0cb01fd) and their tests
(C3.3, e39791d); the monitor's window (C4.1, bc75c74), the page's own
numbers (C4.2, c51d97a), the rows (C4.3, 8366b3d), its rows drawn by its
own worker (C4.1, f331041) and its contract (C4.4, 3ef25c8).

**Gate C4, on the host** (`tools/playwright/quality_monitor_gate.mjs`): the
web server as the board builds it (with the owner's surface, a stand-in for
the owner), the gateway and the encoder (x264) of this branch, the encoder
fed with three changing pictures of 320x180 at 30 a second by a stand-in
for uStreamer; Chromium with QUIC chosen and H.264 the mode; the monitor
open over twelve seconds.

| | The monitor | Counted by the gate | Bound |
| --- | --- | --- | --- |
| Frames drawn a second | 17.92 | 17.97 (the picture's worker) | within 10% |
| Mbit/s that came | 0.898 | 0.880 (the frames the encoder handed to the gateway) | within 10% |
| A frame's age when drawn, median, ms | 15 | 15 (the worker's own ages) | within 10 ms |

The difference is the fragments' headers that the frames' bytes lack
(2%). The board's side came from the encoder's real report: x264 encoding
with the engine chosen (there is none on the host), 16.5 frames a second,
the stages and the gate's rates.

What the open monitor costs the page's main thread, from Chromium's trace
of eight-second windows with it closed and open in turn, seven pairs a
level: the difference of the whole thread's task time is 0.77 ms a second
in Basic and 0.24 in Extended in the median (bound 2); the monitor's own
timer took 0.10 to 0.29 ms a second, and its window drew nothing on the
main thread. Single windows vary by up to three milliseconds a second
(the video path's garbage collection, and once a task of 11 ms in a closed
window), which is why the median of pairs is the measure. With the rows in
the document instead, the same measure gave 4 to 5 ms a second (C4.1,
f331041).

**Gate C4 passed.**

Receipts: `evidence/private/c4-gate-final2.json` (and `c4-gate-final.json`,
`c4-gate-run2.json`, the runs before it).

### C5, 2026-10-04: the board

**Installed** (C5.1): slot 68 (`0.1.0-alpha.13`, source 958c3c7) and the
encoder of this plan (`4e3d9a8c`, the same binary from that source to
1498dce) through `deploy_vp8.sh`, whose drop-in now names the encoders'
reports for the server; slot 69 (`0.1.0-alpha.14`, source 5de8e6c), whose
monitor counts its draws for the probes; slot 71 (`0.1.0-alpha.15`, source
e477b42) with the Speed's places below; and slot 72 (`0.1.0-alpha.16`,
source 1498dce), whose quality answer no longer waits for the update
status (C5.3 below). Every number of the gate here is slot 72's. The
update runtime (dev36) and the driver stayed as they were.

**How it was measured** (C5.2): the target showed a still picture, so the
encoder had a frame every two seconds to encode, most of them key frames.
Every run began with the board quiet, sampled the units' processor time
on the board every five seconds and ended where another address
connected (none did); the monitor was read at every one of its draws.
Over QUIC (`quic_video_device_probe.mjs` with `QUALITY_MONITOR`, through
its relays without loss or delay) the probe counts the frames drawn and
their age by the board's clock; the relays count the QUIC packets whole,
of which on a still picture a quarter is the transport's own (0.023 Mbit/s
against 0.0176 of frames), so the Speed is held, as in gate C4, to the
frames the encoder handed to the gateway: `tools/device/frame_socket_tap.py`
read them from the frame socket beside the gateway (as the gateway's user,
the only one the socket admits) and took those whose capture time lies
within the probe's span. Over WebRTC (`quality_monitor_device_probe.mjs`)
the page's own connection is read with `getStats()` at the span's ends,
and the video element's drawn frames. Over MJPEG the monitor has the
capture's numbers only. The probes count the page's API requests that
failed.

**The Speed's places.** On slot 69 the monitor's Speed over VP8 was 18%
above what `getStats()` counted (0.013 against 0.011 Mbit/s): its values
were 0.00, 0.01 and 0.04 Mbit/s, VP8's small frames of about five
kilobits shown as ten. The bytes were right; two places were too few for
a still screen. Below one megabit a second the Speed has three places
since (C5.2, 724cf3d; "Basic" above).

| Path and engine (slot 72) | Frames a second: the monitor, counted | Mbit/s: the monitor, counted | Age at drawing, median ms: the monitor, the probe | The Codec row |
| --- | --- | --- | --- | --- |
| QUIC, video engine | 0.50, 0.50 | 0.018, 0.0176 (frames) | 49, 47 | H.264 HW 1920x1080 |
| QUIC, x264 | 0.51, 0.50 | 0.021, 0.0199 (frames) | 66, 59 | H.264 SW 1920x1080 |
| WebRTC H.264, video engine | 0.50, 0.50 | 0.017, 0.017 | estimated (`~`) | H.264 HW 1920x1080 |
| WebRTC H.264, x264 | 0.50, 0.50 | 0.021, 0.021 | estimated (`~`) | H.264 SW 1920x1080 |
| WebRTC VP8 | 0.49, 0.48 | 0.011, 0.011 | estimated (`~`) | VP8 1920x1080 |
| MJPEG | - | - | - | MJPEG 1920x1080, JPEG 100, 4:2:2 |

Every run 120 s (MJPEG 60), 121 draws (60) and none missed; over QUIC 62
and 61 frames drawn, none given up, no decoder restarted, and the tap had
the same frames, 40 of them key frames with the engine and 34 with x264.
Slot 71 gave the same within these bounds (`c5-s71-*`).

**The engine from the menu** (`engine_menu_device_probe.mjs`, the monitor
open in Basic): five times each way over QUIC and five over WebRTC, H.264
SW and then H.264 HW clicked in the video menu while the page watched.

| | Over QUIC | Over WebRTC |
| --- | --- | --- |
| x264 encoded after the click, ms | 325 to 386 | 369 to 436 |
| The engine encoded after the click, ms | 377 to 454 | 369 to 464 |
| The monitor's Codec row named it after the click, ms | 496 to 1,837 | 779 to 2,107 |

The picture went on through every switch; the board's choice was
`hardware` before and after. The monitor names the new engine with its
next answer after the encoder's next report: within about two of its
seconds.

**What the open monitor costs the board** (the same QUIC viewer, 60 s
each, closed and open in turn; the units' share of a core over the span):

| | Closed (three runs) | Basic (two) | Extended (one) |
| --- | --- | --- | --- |
| The web server | 0.23 to 0.25 | 0.62 to 0.63 | 0.66 |
| The H.264 encoder | 6.22 to 6.26 | 5.91 to 6.03 | 6.04 |
| nginx | 0.04 to 0.05 | 0.24 | 0.24 |

The web server takes 0.40% of a core more for the monitor's answer once a
second, nginx 0.19%; the encoder took no more (its share goes with the
frames it encodes). The QUIC gateway (0.5), Janus (0.4) and uStreamer
(6.4) did not change.

No API request of the page failed in any of these runs, and the kernel's
log had no line from the start of these measurements (on slot 69) to
their end: 466 lines, the last of them 35 s after the board's boot.

**Gate C5 passed**: the menu's choice changed the engine within 0.46 s
for a watching viewer both ways, over QUIC and WebRTC, and the monitor
named the engine that encoded; the monitor's Basic numbers agree with the
probes within the bounds of gate C4; with the monitor open the web
server's and the encoder's processor time grew by 0.40% of a core (bound
2); no API error and no kernel line.

**Regression** (C5.3):

- **A slot from before this plan with the new encoder.** The update path
  takes a slot only with a sequence above the one confirmed, so slot 66
  could not be installed again; the code of 6522ff5 (the end of the
  hardware H.264 plan, slot 66's source b31149a in everything a slot
  holds) was built as slot 70 (`0.1.0-alpha.11`) and installed. Its server
  sends no `engine` line, and the encoder of this plan kept its own choice,
  the engine: over QUIC 31 frames of the still picture, 50 ms old in the
  median when drawn, none given up; over WebRTC 16 frames at 1920x1080,
  none lost; `h264Encoder: hardware`.
- **On slot 71** the GUI walks with the monitor closed (desktop, mobile)
  and open in Basic on a phone passed; open on the desktop (Basic,
  Extended) they failed on one request each: `GET /api/vm/quality`, given
  up by the monitor after 0.9 s while the settings' System tab loaded. A
  fresh answer took 0.51 to 0.61 s, of which nearly all was the update
  runtime's status (`GET /api/application-update/status` alone: 0.50 to
  0.62 s); the WiFi scan that the tab makes answered in 86 ms and held
  nothing up. Since 1498dce the version is read in the background
  ("`GET /api/vm/quality`" above), and on slot 72 a fresh answer took 11
  to 20 ms. The firewall probe stopped on slot 71 at the video menu, where
  it found two H.264 entries instead of one (8ffb18c), after its checks of
  Restricted's rules and session and before Paranoid; it restored
  Baseline.
- **On slot 72**: the GUI walks, desktop and mobile, with the monitor
  closed, open in Basic and (desktop) in Extended: five walks of 21 steps,
  no failed request and no page error, the monitor open at the end where
  it was opened (the open ones made 124 to 133 requests, the closed 61 to
  64). VP8 and H.264 over WebRTC (`vp8_webrtc_probe.mjs`) decoded 15
  frames each at 1920x1080 with no packet lost (Chromium dropped one VP8
  frame). The firewall probe passed: a QUIC
  session in Baseline and Restricted with the page's two H.264 entries over
  QUIC and VP8 closed, none in Paranoid, Baseline again at the end. MJPEG
  (`tools/device/stream_latency_probe.py`): the viewer at full speed had
  51.5 to 52.5 frames a second and the slow one 40.3, as before; the slow
  one trailed the full-speed one by 242 to 256 ms in the median in every
  run (249 to 255 on slot 71, 265 to 271 on slot 66). The probe's limit of
  300 ms in 90 of 100 was met in three runs of three on slot 71 and two of
  six on slot 72, where the full-speed viewer's median moved from -46 to
  +121 ms between runs: the probe turned this host's monotonic times into
  true time with SNTP readings before and after a run only, and the WSL
  host's monotonic clock changed its rate within a run (1.030 to 1.047 of
  true time). Since 7842d22 it reads SNTP every two seconds; three runs
  then gave 11 to 12 ms for the full-speed viewer in the median and 280 to
  286 ms for the slow one in 90 of 100, all three within the limit.

**`check.sh`**: green before every slot build. Once, on e477b42, a test of
the frame socket from the QUIC plan
(`on_a_still_picture_a_key_frame_comes_when_it_is_asked_for`) got a delta
frame where it waited for the periodic key frame while slot 70 was built
beside it on the same host: a load flake, green when run again alone.

Receipts: `evidence/private/c5-s72-*` (the gate and the regression on slot
72), `c5-s71-*` (slot 71), `c5-old70-*` (slot 70), `c5-quic-*`,
`c5-webrtc-*`, `c5-mjpeg-basic*` (slot 69), `c5-s71-quality-vs-scan.json`,
`c5-s71-read-times.json`, `c5-s72-read-times.json`,
`update-to-seq68-*`, `-seq69-*`, `-seq70-*`, `-seq71-*`, `-seq72-*`,
`c5-state-after-20261004.json`.

### Review fixes, 2026-10-04

The review of the work above (`docs/baselines/audit-20261004-review.md`) and
its fixes (`docs/baselines/work-plan-20261004-review-fixes.md`). What changed
against the design is written here with its reason, and each gate's numbers
as it is passed.

**Gate B, the page.** Changed against the design:

- The monitor needs workers and `OffscreenCanvas` (Q1). The design drew the
  rows in a worker and said nothing of a browser that cannot, or refuses, one.
  Such a browser looped the monitor through 22,000 to 30,000 canvases in three
  seconds, and where the worker was refused the router's error screen replaced
  the whole page, at every load, because the open monitor is remembered. The
  window now says in one line that it needs workers and `OffscreenCanvas` and
  does nothing else: no rows, no timer, no request. A drawing fallback on the
  main thread is not built, since it would bring back the 4 to 5 ms a second
  that the worker removed, and a second renderer to keep right.
- The Direct path has no encoder of ours (Q9): the Codec row shows `H.264` and
  the picture's size, the Bitrate row a dash, and Extended has no encoder
  rows, instead of another viewer's engine and bitrate.
- The rows follow the display's pixel ratio (Q11): zooming the page or moving
  it to another display draws them again at the new ratio instead of leaving
  them blurred.

Numbers (`quality_monitor_contract.mjs`, `tools/test_quality_monitor.mjs`,
`quality_monitor_gate.mjs`):

| | Before | After |
| --- | --- | --- |
| Canvases made in three seconds without `transferControlToOffscreen` | 30,430 | at most one, none left |
| The page with the worker refused | the router's error screen | the toolbar and the note, no error screen |
| The rows' pixels after the ratio goes from 1 to 2 | 264 wide, unchanged | 528 within three seconds |
| The Direct path's Codec row | `H.264 HW 1920x1080` (another viewer's engine) | `H.264 1920x1080` |

The monitor's pure functions have unit tests now (ten cases in
`tools/test_quality_monitor.mjs`, in `check.sh`). The host gate again, with
QUIC, the real server, gateway and encoder (x264) over twelve seconds:
17.75 frames a second shown against 17.73 counted, 0.741 Mbit/s against 0.720,
a frame's age 4 ms against 4; the open monitor costs the page's main thread
0.09 ms a second in Basic and 0.42 in Extended, the median of seven pairs
(bound 2; gate C4 had 0.77 and 0.24).

**Gate B passed.** Receipts: `evidence/private/review-fixes-gate-b.json`.

**Gate C, the server.** Changed against the design:

- The quality answer makes no rate across a pause (Q5; "Refined in the
  review fixes" under `GET /api/vm/quality`).
- The engine choice is offered only with the encoder's report configured, and
  what is not offered is not served (Q3, Q7; the text about the right under
  "The choice and where it lives"). A slot on a board whose encoder or drop-in
  is older no longer offers H.264 HW and H.264 SW that change nothing; and the
  engine choice and the WebRTC quality, which only the manifest hid where the
  video controls were off, are refused by the server there too.
- A choice is stored and sent under one lock (Q8), and a report's time is
  read from the file that was read (Q10).

Numbers:

| | Before | After |
| --- | --- | --- |
| The first quality answer after a pause | an encoder rate over the pause (60 frames a second in the test) | no rate |
| Two engine choices at once: the file and the channel differ | 50 of 50 repetitions (33 to 40 of 4000 rounds each) | 0 of 50 |
| A report read while it is replaced: a time that is not its file's | 20 of 20 repetitions (141 to 197 of about 10,900 reads) | 0 of 50 |
| `the_quality_answer_is_built_once_a_second_from_the_reports` beside 96 busy loops | 2 of 12 runs failed | 30 of 30 passed |
| The plan's server tests together beside 96 busy loops | 2 of 12 runs failed | 30 of 30 passed |

The manifest and the answers for the four combinations of the video controls
and the report are held by
`the_servers_own_video_settings_are_offered_and_served_only_where_configured`;
the first of its assertions failed before (the engine was offered without the
report), and with the controls off both settings were served.

**Gate C passed.** Receipts: `evidence/private/review-fixes-20261004/baseline`
and `gate-c`.
