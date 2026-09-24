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
