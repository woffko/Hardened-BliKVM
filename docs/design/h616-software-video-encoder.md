# H616 software video encoder

Status: design and feasibility measurement. No encoder is implemented or
accepted. Replaces the dependency on the vendor LIBX264-VIDEO uStreamer binary,
whose source could not be located (see
[H616 H.264 source contract](h616-h264-source-contract.md)).

## Hardware facts (read-only device inspection, 2026-09-24)

- CPU: 4x Cortex-A53 at 1.512 GHz, 984 MiB RAM.
- `/dev/video0` is `cedrus`, a stateless *decoder* (MPEG-2, H.264, HEVC, VP8
  output formats only). The mainline driver exposes no encoder; the vendor
  CedarX encoder needs proprietary blobs and is not a reproducible option.
- Capture is the MS2131 USB device (`/dev/video1`, `/dev/video2`) delivering
  MJPEG; the existing uStreamer instance remains its only reader.

## Codec feasibility

Measured with the device's ffmpeg on synthetic full-motion input (worst case;
receipt `evidence/private/encoder-benchmark-20260924-a.json`). 150 frames equal
five seconds of 30 fps video; generating 1080p input alone took about 3 s.

| Codec (realtime settings) | Size | CPU seconds | Wall seconds | Verdict |
| --- | --- | --- | --- | --- |
| VP8 (libvpx, cpu-used 16) | 1280x720 | 8.8 | 4.2 | Real time with headroom (about 1.8 cores) |
| VP8 | 1920x1080 | 19.6 | 9.2 | About 16 fps; all cores |
| H.264 (x264 ultrafast) | 1920x1080 | 14.9 | 6.9 | About 22 fps; patent-encumbered |
| VP9 (cpu-used 8, row-mt) | 1280x720 | 23.4 | 8.5 | About 18 fps; too heavy |
| AV1 (libaom realtime) | 1280x720 | - | - | Did not complete; unsuitable |

Desktop video is mostly static; skipping unchanged frames makes typical load
far lower than this worst case. MJPEG decoding of the capture adds cost that
is not included above.

## Decision

- Use VP8 (royalty-free, BSD-licensed libvpx, mandatory WebRTC codec supported
  by all target browsers). Default to 1280x720 at up to 30 fps; allow 1080p
  only at a reduced frame rate. MJPEG stays the full-resolution path.
- AV1 and VP9 are rejected on this CPU. H.264 is not pursued without a
  reproducible encoder source and a patent/licensing decision.

## Architecture

1. Input: the existing uStreamer MJPEG stream over its local socket (single
   capture owner, no second reader of the MS2131).
2. Unchanged-frame skip on the compressed JPEG bytes, then JPEG decode to I420
   (libjpeg-turbo) and VP8 realtime encode (libvpx) with a bounded queue of one
   frame; the newest frame wins under overload.
3. RTP VP8 packetization to a loopback UDP port consumed by the Janus streaming
   plugin, which already serves WebRTC on the device. Signaling stays behind the
   authenticated server; Janus binds to loopback only.
4. A separate unprivileged service with its own sandbox; bounded CPU via
   `CPUQuota=` and a thermal/fps governor.

## Acceptance

Measured fps, bitrate, CPU, memory, temperature and glass-to-glass latency on
the device with real capture, desktop and mobile browsers, MJPEG/VP8 switching,
and failure isolation (encoder crash does not affect MJPEG or HID).

## Implementation status (2026-09-24)

- `scripts/build-libvpx.sh` builds a static VP8-encoder-only libvpx 1.15.2 from
  a pinned SHA-256, cross-compiled with clang against pinned musl 1.2.5 headers.
  ARMv8.2+ extensions (dotprod, i8mm, SVE) are disabled because runtime CPU
  detection is off and the Cortex-A53 faults on them (observed as SIGILL
  before this was fixed). Two aarch64 builds produced identical archives.
- `bli-kvm-video-encoder` (feature `vp8`): bounded uStreamer MJPEG reader,
  frame gate, JPEG to I420 (pure-Rust zune-jpeg with NEON, optional 2x
  downscale), a narrow C shim around libvpx, RFC 7741 RTP to loopback, and a
  three-stage pipeline (read, decode/convert, encode) with newest-frame slots.

Device measurement with pre-generated full-motion testsrc2 JPEG at 30 fps fed
through a local fake uStreamer socket (worst case; the live desktop was static
and produced about 11 frames in 25 s through uStreamer's drop-same-frames):

| Input | Output | fps | Encoder cores | RSS | Bitrate |
| --- | --- | --- | --- | --- | --- |
| 1280x720 | 1280x720 | 22.0 | 1.57 | 37 MiB | 3.6 Mbit/s |
| 1920x1080 | 960x540 (halved) | 20.1 | 1.98 | 25 MiB | 3.3 Mbit/s |
| 1920x1080 | 1920x1080 | 12.8 | 2.88 | 80 MiB | 2.8 Mbit/s |

Per-frame stage times at 720p: JPEG decode 14 ms, conversion 9 ms, VP8 encode
23 ms. Peak SoC temperature during the runs was 69 C. Defaults: 720p capture at
full size, or 1080p capture halved. Full 1080p VP8 is not offered.

Signaling and deployment (host-complete, not yet deployed):

- `/api/stream/webrtc` is an authenticated, origin-checked WebSocket with a
  closed offer/answer/candidate protocol. The server alone talks to the
  distribution Janus 0.11 over its loopback WebSocket (one session and
  streaming handle per viewer, mountpoint 1, keepalive, destroy on exit, two
  viewers at most). The route answers only with `BLI_KVM_JANUS_WS` set to a
  loopback address, and only then is `web_rtc_janus` advertised; MJPEG stays
  preferred. The GUI offers "VP8 (WebRTC)".
- `packaging/janus/` enables only the streaming plugin and the loopback
  WebSocket transport, no admin API, host candidates only, RTP ports
  20000-20099 for media.
- `hardened-blikvm-janus.service` and `hardened-blikvm-video-encoder.service`
  run as dynamic users; the encoder has loopback-only networking and
  `CPUQuota=250%`.
- `tools/device/deploy_vp8.sh install|remove` installs the encoder binary,
  Janus configuration, units and the server drop-in.

A transient run of the distribution Janus on the test board with this
configuration loaded only the streaming plugin and the WebSocket transport
(port 8188); the HTTP transport and all other plugins stayed disabled. The
same run also showed that Janus silently falls back to all plugins and an
HTTP listener when its configuration cannot be read, so the unit requires the
configuration file and restricts binds to the signaling port and media range.

Open: deployment needs the application slot built from current source (the
installed server predates the signaling route); the Restricted and Paranoid
firewall modes need `webrtcMedia: [20000, 20099]` in the reviewed firewall
configuration (host-implemented and checked against the device's nft) before
VP8 works in those modes; acceptance with real capture on desktop and mobile
browsers, glass-to-glass latency and MJPEG/VP8 switching.

## Encoder on demand and the MJPEG relay (2026-09-28, plan phase J)

- The encoder ran whether or not anyone watched VP8: it decoded and encoded
  every changed frame and took 2.3 of the four cores while the operator
  watched MJPEG. It now listens on `/run/hardened-blikvm-vp8/demand.sock`
  (`BLI_KVM_VP8_DEMAND_SOCKET`, group `hardened-blikvm`, mode 0660). The
  server (`BLI_KVM_WEBRTC_DEMAND_SOCKET`, set by `deploy_vp8.sh` next to
  `BLI_KVM_JANUS_WS`) holds one connection per viewer for as long as the
  viewer's signaling lasts and names the codec on its first line. Without a
  connection the encoder leaves the capture stream two seconds later; a new
  viewer makes it reconnect and start from a key frame. Without the socket
  setting the encoder runs always, as before.
- MJPEG: the server reads the capture's multipart stream continuously and
  keeps only the newest complete part (`mjpeg_latest.rs`); the response
  sends a part only when the connection takes one, so a slow viewer skips
  frames instead of receiving old ones late. That works only if the queues
  after the server stay small. Measured on the board with a viewer reading
  at 20 Mbit/s (`tools/device/stream_latency_probe.py`): nginx's loopback
  TCP connection to the server grew its receive buffer to 4-6 MB of frames
  (the kernel's receive autotuning; nginx cannot set `SO_RCVBUF` on
  upstream connections, and a route `window` clamp is overridden by the
  autotuning), and the viewer trailed by 2.9 s while a full-speed viewer
  trailed by 39 ms at p90. nginx therefore takes the stream from the
  server's unix socket (`BLI_KVM_STREAM_SOCKET`,
  `/run/hardened-blikvm-stream/mjpeg.sock`, mode 0666 like the reachable
  TCP port, with the same session check), whose send buffer the server sets
  to 64 KiB per connection. `net.ipv4.tcp_notsent_lowat` of 64 KiB
  (`packaging/sysctl/60-hardened-blikvm-stream.conf`) keeps nginx's send
  queue to the browser under a frame. Measured with 60 KB frames: a
  full-speed viewer 12 ms behind uStreamer at p90, a 20 Mbit/s viewer 2.9 s
  over TCP, 310 ms over the unix socket with a 128 KiB limit and 288 ms with
  64 KiB (`mjpeg-latency-seq54-*.json`).
- uStreamer's `X-Timestamp` is the board's wall clock when the frame left
  it (checked on the board to 1 ms), so the probe measures a viewer's delay
  as its arrival in true time minus the stamp. The build host's monotonic
  clock ran at a steady 0.9 of true time, and its wall clock was stepped, so
  the probe converts arrivals through two SNTP readings.
- The capture unit's default rate is 30 frames/s, as in the reference; the
  SD image drops the source board's saved video settings.

## H.264 through x264 and the codec benchmark (2026-09-28, plan J2-J4)

- `scripts/build-x264.sh` builds a static 8-bit 4:2:0 x264 (build 165, the
  head of its stable branch, pinned by commit and SHA-256) with its NEON
  assembly for the board and without assembly for host tests. x264 is
  GPL-2.0-or-later, compatible with the project's GPL-3.0-only; unlike VP8,
  H.264 is covered by patent pools, which is why VP8 came first.
- The encoder (`bli-kvm-video-encoder`, features `vp8` and `h264`) runs one
  codec per instance: `BLI_KVM_ENCODER_CODEC` is `vp8`, `h264` (superfast)
  or `h264-ultrafast`; x264 uses tune `zerolatency`, the constrained
  baseline profile, SPS and PPS before every key frame, and constant-rate
  input (with variable-rate input x264 held each frame until the next one
  arrived, and a still screen sends no next one). RTP follows RFC 6184
  (single NAL units, FU-A fragments). `hardened-blikvm-video-encoder-h264`
  feeds Janus mountpoint 2 (port 5006, payload type 97) and has its own
  demand socket; the server offers `web_rtc_janus_h264` when
  `BLI_KVM_WEBRTC_H264_DEMAND_SOCKET` is set, and the browser asks for it
  with `/api/stream/webrtc?codec=h264` ("H.264 (WebRTC)" in the video menu).
- `bli-kvm-video-encoder bench [FRAMES]` reads capture frames into memory
  (they never leave the process), shifts each by 24 more rows to stand for
  scrolling, and times every codec and size on the same frames. On the
  idle board, 60 frames of 1920x1080 (`codec-bench-*.json`):

  | codec | size | decode + convert | encode | frames/s, two converters |
  | --- | --- | --- | --- | --- |
  | VP8 | 1920x1080 | 25 + 20 ms | 39-43 ms | 23-25 |
  | x264 superfast | 1920x1080 | 25 + 20 ms | 45-47 ms | 21-22 |
  | x264 ultrafast | 1920x1080 | 25 + 20 ms | 29 ms | 34 |
  | VP8 | 1280x720 | 25 + 22 ms | 15 ms | 42 |
  | x264 ultrafast | 1280x720 | 25 + 22 ms | 13-14 ms | 42 |

  The 10 frames/s seen earlier came from load (the idle encoder, MJPEG at
  60 frames/s and its TLS), not from the codecs. Preparing a frame (45 ms)
  was the bottleneck, so two converter threads take turns and the encoder
  drops a frame that finishes after a newer one. x264 ultrafast at
  1920x1080 is the fastest full-size WebRTC mode; 1280x720 is taken from the
  capture (Settings > Video resolution) rather than scaled, since scaling
  after decoding a 1080p frame saves no preparation time.
- Key frames: the encoder forces one at least every two seconds, besides one
  for every new viewer. Frames come only when the picture changes, so a
  frame-count interval (60 frames) meant up to two minutes between key
  frames on a still screen; Janus cannot pass a viewer's key frame request
  on to an RTP source, and a viewer that lost the one key frame, or joined
  before its media was up, saw nothing. A 1920x1080 VP8 viewer on a still
  screen then decoded 12 frames in 20 s, 8 of them key frames
  (`vp8-webrtc-seq55-key-*.json`).
- CPU on the board (`cpu-modes-seq55-*.txt`, share of all four cores, still
  screen): no viewer 1.5 % (the encoders wait), MJPEG 7 % (nginx, server,
  uStreamer), VP8 10 % (encoder 13 % of a core), H.264 11 % (encoder 12 %
  of a core), 1.5 % again once the viewers left.
- Delay of a WebRTC frame, on a still screen where every frame is a key
  frame (`{vp8,h264}-webrtc-seq55-diag-*.json` and the encoders' journal
  lines of the same minute): on the board, decoding the JPEG, converting it
  and encoding take 105-120 ms for H.264 (34-46, 28-30 and 41-44 ms) and
  135-155 ms for VP8 (37-38, 26 and 73-92 ms); in the browser (Chromium,
  1920x1080, no packet lost) the frame waits 8 ms in the jitter buffer and
  is decoded 10 ms after its first packet arrived (H.264 1.4 ms of decoding,
  VP8 3 ms); the round trip was 1-12 ms. A frame that is not a key frame
  encodes faster (the benchmark's 29 ms for x264 ultrafast, 39-43 ms for
  VP8): with the benchmark's 45 ms of preparation that is about 85 ms
  (H.264) and 100 ms (VP8) from capture to screen, and 20-30 ms more when
  preparation takes the 60-75 ms seen here. MJPEG's
  12 ms at p90 is measured from the capture's own timestamp to the viewer
  and leaves the JPEG decoding to the browser.
- The probes' Chromium decodes H.264: the first H.264 probe (before the
  periodic key frames) received only the headers and decoded nothing, as
  the VP8 probe of the same minute did; with key frames every two seconds
  both decode at 1920x1080.

## Frame preparation with libjpeg-turbo (2026-10-01)

- Preparing a capture frame (decoding the JPEG, then 4:4:4 to I420) took
  45 ms at 1920x1080 and was the slowest stage. The encoder now decodes with
  libjpeg-turbo 3.2.0 (`scripts/build-libjpeg-turbo.sh`: pinned, signature
  checked, NEON required) straight into Y, Cb and Cr planes at the frame's
  own subsampling, so a 4:2:2 capture needs only its chroma rows averaged
  for I420; with halving it scales by half in the IDCT. zune-jpeg stays as
  the fallback feature (`zune`), and `bench` measures both decoders on the
  same frames.
- On the board (`jpeg-bench-*.txt`, 60 frames of the same capture, both
  decoders in one run):

  | capture | decoder | decode | convert | total |
  | --- | --- | --- | --- | --- |
  | 1920x1080 4:2:2, 256 KB frames | zune-jpeg | 25.1 ms | 20.2 ms | 45.3 ms |
  | 1920x1080 4:2:2, 256 KB frames | libjpeg-turbo | 19.1 ms | 6.1 ms | 25.2 ms |
  | quality 100, 355 KB frames | zune-jpeg | 45.8 ms | 21.4 ms | 67.2 ms |
  | quality 100, 355 KB frames | libjpeg-turbo | 28.8 ms | 5.9 ms | 34.7 ms |

  At 1280x720 (decoded at 1920x1080, then scaled by two thirds) the totals
  are 27.7 against 48.1 ms. The fast IDCT saves about 1 ms and is not used.
  With two converter threads, preparation no longer bounds any codec at
  1920x1080: x264 ultrafast reaches 36 frames/s (bounded by its 28 ms of
  encoding), VP8 25 frames/s, at the earlier capture settings.
- Quality: luma matches the zune path within 70 dB, and 4:4:4 chroma
  exactly. For subsampled frames the new chroma is closer to the source
  picture (tests: 4:2:2 49.8 against 38.2 dB, 4:2:0 44.3 against 36.1 dB),
  because zune-jpeg upsamples chroma before it is averaged back.
- Two faults found while testing, both handled: a damaged frame could leave
  the TurboJPEG decompressor's Huffman tables corrupt, so every later frame
  failed ("Bogus Huffman table definition") until a restart, and the
  decompressor is now recreated after any error; and the fast IDCT scaled
  by half returned wrong chroma for 4:2:0 frames, so it is never used when
  halving. 20000 randomly damaged frames run through the decoder in a
  release build in `check.sh`.
- Live, with a WebRTC viewer (`stage-cpu-*-jpeg-*.txt`): the measurement
  after the change ran with the capture settings the operator had chosen in
  the meantime (60 frames/s, quality 100, frame detection off), which a
  restart of uStreamer applied. The probe page then also pulls every MJPEG
  frame, so nginx, the server and the video proxy take 40-50 % of a core
  each and the encoders' stages slow down by contention (decode 35-84 ms,
  convert 10-20 ms per frame). Those numbers are not comparable with the
  earlier still-screen run (decode 26-49 ms, convert 20-29 ms); the bench
  above is the controlled comparison.
