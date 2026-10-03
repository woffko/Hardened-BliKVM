# H.264 by engine in the video menu, and a quality monitor

Plan: `docs/baselines/work-plan-20261003-h264-choice-quality-monitor.md`.
This note fixes what the plan leaves to its design: the API's fields with
their units and bounds, the encoder's report, the demand line, the menu's
cases, the monitor's rows and where each comes from, the reference and the
licence question. The record of what was measured follows at its end as the
work goes on.

Status (2026-10-03): the engine as the board's choice is built (C1) and
passed its gate on the test board; the menu and the monitor are not built
yet.

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
  where they offer `webrtc_bitrate` and the H.264 demand socket is
  configured.
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
one already made. Lines stay below the encoder's bound of 32 bytes. The
VP8 encoder never gets the line.

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
- `h264HardwareAvailable: false`: H.264 HW is shown disabled, with the
  encoder's reason in its tooltip.
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

### Basic

| Row | Shows | Where it comes from |
| --- | --- | --- |
| Speed | what arrives, Mbit/s | WebRTC: `bytesReceived` of the video's inbound RTP; QUIC: the session's bytes; MJPEG: - |
| Delay | a frame's age when it is drawn, ms | QUIC: measured by the board's clock (the session already has it); WebRTC: estimated, the jitter buffer's delay a frame and half the round trip, marked `~`; MJPEG: - |
| Path | MJPEG, H.264 Direct, WebRTC or QUIC | the page's mode and transport |
| FPS | frames drawn a second / the capture's | the page's count; the board's capture rate |
| Bitrate | the target, kbit/s | the WebRTC quality (both encoders and QUIC); MJPEG: the JPEG quality instead |
| Codec | H.264 HW, H.264 SW, VP8 or MJPEG, and the picture's size | the encoder's report, the page's decoder |
| Chroma | 4:2:0 for H.264 and VP8, 4:2:2 for the capture's JPEG | the codec |

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
- `version`: the slot's.

## The page's side

- **QUIC**: the session's own counters, which `useQuicVideo` and the
  receiver already keep: frames, their age at drawing, given up, key-frame
  requests, round trip, lost datagrams, the gateway's counters.
- **WebRTC**: `RTCPeerConnection.getStats()` once a second while the
  monitor is open: the video's `inbound-rtp`, its `codec`, the selected
  `candidate-pair`.
- **H.264 Direct**: the frames its decoder takes and gives, where the board
  offers the mode.
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
  a choice that is not what encodes, no engine, no right; the monitor's
  window, its two levels, its rows for each path against the stand-in, the
  API asked only while it is open.

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
