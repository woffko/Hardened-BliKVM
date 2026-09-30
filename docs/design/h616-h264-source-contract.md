# H616 H.264 source contract

## Reviewed sources

The operator-provided source map is
`/mnt/d/debug/blikvm_v4_video_settings_source_map_for_agent.md`.
The following public revisions were resolved and inspected on 2026-09-20:

| Repository | Revision | Observed evidence |
| --- | --- | --- |
| `blikvm/blikvm` | `3e34c04a1dfa84636050585d12d5e585098258ea` | Package tree contains an H616 uStreamer binary, video launcher and Janus installer. |
| `blikvm/blikvm-web-server` | `cecede74ebe75239576c5eebf2eed7c4003a26fa` | `lib/kvmd-video.sh` contains the H616 software-encoder launch path; `lib/h616/janus_configs/janus.plugin.ustreamer.jcfg` names its matching video sink. |
| `blikvm/ustreamer` master | `05a5d3fed418503c49aa42916874c8a9a845c04b` | `src/ustreamer/encoder.h` declares CPU, HW, M2M-VIDEO and M2M-IMAGE, not LIBX264-VIDEO. |
| `blikvm/ustreamer` m2m | `91e16c0ed8aa67328cca5f3daff39636551ad303` | Encoder header also lacks LIBX264-VIDEO. |

`ThomasVon2021/ustreamer` HEAD resolves to the same master revision above.
A commit-message search for libx264 in `blikvm/ustreamer` returned no matches;
that is not proof that no implementation exists elsewhere or in other history.

## Confirmed newer vendor launch contract

Additional public-branch review on 2026-09-23 enumerated four branches via the
GitHub API. Master and m2m still resolve to the revisions above. The remaining
branches were inspected at immutable commits:

| Branch | Revision | Bounded source evidence |
| --- | --- | --- |
| `shm` | `ccf713dc1cbee167c5eccbc3396febcb2e13f876` | `src/ustreamer/encoder.c` dispatches CPU, HW, optional OMX and optional NOOP; its type parser has no LIBX264-VIDEO entry. |
| `semwait` | `1b2de094380a95cda7556b8d2c7e735b4b57ab8d` | `src/encoder.h` declares CPU, HW and optional OMX, without a libx264 encoder type. |

Repository-scoped GitHub code search for libx264, including forks, returned no
hits. Search indexing is incomplete evidence; the pinned encoder source is the
stronger evidence for these two branch entrypoints. This review does not establish
absence from every historical commit, fork or unpublished vendor source, and does
not resolve the delivered H616 binary's reproducible source provenance.

The binary history identifies commit
`77a1c1f5ad317738db9c6d210d6285a085861a84` (2025-05-15), whose message says
"v4 add software h264 encode". Its changed-file list contains delivered binaries,
configs and launch/application updates, not the embedded libx264 C source. This
pins introduction provenance but does not establish a reproducible source build.

The `MangoPi Mcore` branch locates a JPEG-capable V4L2 device and invokes:

```text
--format=MJPEG --encoder=LIBX264-VIDEO --device=<selected capture>
--resolution=<requested mode> --desired-fps=<requested fps>
--h264-bitrate=<kbps> --h264-gop=<gop>
--drop-same-frames=30 --h264-sink=demo::ustreamer::h264
```

The matching Janus plugin config uses `sink = "demo::ustreamer::h264"`.
The reviewed H616 branch does not pass `--quality`. This agrees with the source
map's warning that a generic quality field is not evidence of V4 JPEG-quality
control. The vendor's no-JPEG-device fallback omits the software H.264 path.

Names such as `kvmd-video.sh` are historical vendor filenames, not evidence that
KVMD is present on the target. The installed older vendor launcher observations
in `docs/baselines/video-settings-source-map.md` remain historical evidence of
that installed revision; they do not rule out porting this newer vendor path.

## Adaptation requirements still open

Public upstream issue [blikvm/blikvm#363](https://github.com/blikvm/blikvm/issues/363),
inspected on 2026-09-23, reports an H616 upgrade to v2.2.3-alpha where the delivered
encoder exits with code 127 because `libx264.so.163` is missing. This is an upstream
user report, not a reproduced result on our target. It corroborates the dependency
observed in our static binary inspection and makes dependency closure an explicit
release gate: resolve the packaged encoder's exact ELF NEEDED entries against the
assembled target rootfs without executing the target binary on the build host,
then verify startup and decoded video on the separate acceptance system. Merely
shipping an encoder executable and a systemd unit is insufficient.

The branch API still returns the same four revisions listed above. A bounded
global GitHub code search for `LIBX264-VIDEO` found the already-pinned vendor
launcher but no encoder implementation among its returned matches. The tags API
was initially enumerated without inspecting tagged source trees. A subsequent
bounded tag inspection on 2026-09-23 checked `v4.11` at immutable commit
`496c9300fc3dcb8ed5aec0da0c5c8eb78c0bc386`:

- `src/ustreamer/encoder.h` declares CPU, HW, optional OMX and NOOP.
- `src/ustreamer/encoder.c` has that same closed type-parser/dispatch table;
  `LIBX264-VIDEO` is not accepted there.
- The top-level `Makefile` delegates application builds to `src`, with optional
  Python and Janus targets. It does not identify the missing libx264 backend.

Pinned source URLs:

- https://github.com/blikvm/ustreamer/blob/496c9300fc3dcb8ed5aec0da0c5c8eb78c0bc386/src/ustreamer/encoder.h
- https://github.com/blikvm/ustreamer/blob/496c9300fc3dcb8ed5aec0da0c5c8eb78c0bc386/src/ustreamer/encoder.c
- https://github.com/blikvm/ustreamer/blob/496c9300fc3dcb8ed5aec0da0c5c8eb78c0bc386/Makefile

This rules out that inspected tag's encoder entrypoint as the source of the
delivered LIBX264-VIDEO implementation. It is not an exhaustive tag/history/fork
search and does not prove that the source is unavailable elsewhere. No build or
target execution was performed during this source-only inspection.

Static artifact inspection now confirms that the newer delivered H616 binary
contains the software encoder even though its implementation was not found in
the inspected source branches. The downloaded Git blob matches the tree entry;
ELF metadata identifies AArch64 and a dependency on `libx264.so.163`, and symbols
include `us_libx264_encoder_init`, `us_libx264_encoder_compress` and
`x264_encoder_open_163`. The embedded source path is
`ustreamer/encoders/libx264/libx264.c`. The matching Janus plugin is also AArch64;
its audio-related dynamic dependencies do not authorize enabling audio devices.

Exact blob IDs, SHA-256 hashes, sizes and needed libraries are recorded in
`docs/baselines/h616-vendor-artifacts.json`. Artifacts are retained on the D:
build volume under `vendor-review`, outside the repository, and were inspected
with `readelf`/`strings`, never executed or loaded with `ldd`. These facts prove
binary feature presence, not successful capture/encoding, ABI compatibility with
the target's installed libraries, or reproducibility from source.

- Establish the source/build provenance, license/dependencies and exact feature
  set of the supplied H616 binary that actually supports LIBX264-VIDEO. The
  inspected public uStreamer headers do not establish that implementation.
- Preserve one owner of the MS2131 capture. Do not launch a second reader merely
  to add H.264, or confuse a browser transport switch with encoder configuration.
- Retain the hardened socket/bind and service boundaries rather than copying
  vendor `0.0.0.0` listeners, `eval`, force-kill or world-writable installation.
- Verify the shared-memory ABI and lifetime against the matching Janus plugin;
  match video sink only. Audio sections in the vendor config are not proof of
  supported audio devices or permission to claim UAC2/other device nodes.
- Determine authenticated signaling/media routing and the separate H.264 Direct
  contract. The reviewed path proves Janus/WebRTC, not Direct delivery.
- Verify MS2131 formats/intervals on the target rather than treating the UI's
  nine resolution choices as capture capability evidence.
- Build and measure actual frames, bitrate, CPU/memory/temperature, latency and
  MJPEG/H.264 switching before advertising H.264 capability.

No vendor installer or downloaded binary was executed during this inspection.
H.264 build, integration and hardware acceptance are not complete.
