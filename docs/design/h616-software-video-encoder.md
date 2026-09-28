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
  frames instead of receiving old ones late. `net.ipv4.tcp_notsent_lowat`
  of 128 KiB (`packaging/sysctl/60-hardened-blikvm-stream.conf`) keeps the
  backlog in the kernel's send queues, between the server and nginx and
  between nginx and the browser, to about one frame.
- The capture unit's default rate is 30 frames/s, as in the reference; the
  SD image drops the source board's saved video settings.
