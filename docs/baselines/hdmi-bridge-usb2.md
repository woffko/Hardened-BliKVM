# HDMI capture bridge and video modes (H616 test board)

Evidence gathered read-only on 2026-09-19 while answering the operator's
question about picture quality and H.264.

## The bridge

`/dev/kvmd-video` is a symlink to `/dev/video1`, a **USB2 UVC capture device**:

```
Bus 001 Device 002: ID 345f:2131 MACROSILICON USB2 Video
/sys/class/video4linux/video2/name = "USB2 Video: USB2 Video"
speed = 480 Mbps (USB 2.0 High Speed)
```

The kernel side is plain `uvcvideo`; there is no vendor capture module, no
module parameter file and no sysfs knob on this board.

## What the device offers

`v4l2-ctl --list-formats-ext -d /dev/video1`:

- two uncompressed/MJPEG-carrying formats, no H.264 at all (`grep -c H264` is 0);
- discrete frame rates per size, e.g. `1280x720`: 60, 50, 30, 20 and 10 fps;
- the vendor's own default resolution is `1920x1080` with a documented list
  down to `640x480` (`/etc/kvmd/main.yaml`, `streamer.resolution`).

## Why the quality switch cannot work

The vendor configuration settles it: `streamer.quality: 0` and the ustreamer
command contains **no `--quality` flag** (`/etc/kvmd/main.yaml`), and the engine
reports `encoder: {type: HW, quality: 0}`. The source hands over finished MJPEG
frames, so there is nothing to re-encode and the JPEG quality is whatever the
bridge produced. This is why the setting was hidden in our interface: it was
applied to the engine and changed nothing.

## H.264

- The bridge does not provide H.264 (`grep -c H264` on the format list is 0).
- The SoC (Allwinner `sun50i-h616`, MangoPi MCore) exposes a hardware
  **decoder** only: `/dev/video0` is `cedrus`, the only video driver in the
  kernel configuration is `CONFIG_VIDEO_SUNXI_CEDRUS=m`, and there is no
  encoder character device, no `mpp`/`ve` module and no `/dev/mpp*`. The vendor
  stack never encodes H.264 either: `/etc/kvmd/main.yaml` only ever runs
  `ustreamer --format=mjpeg`.
- Consequently H.264 on this board is **software encoding** on four A53 cores
  while the source stays MJPEG at USB2 rates - it would lower bandwidth for a
  viewer but cannot improve the picture and costs most of the CPU.

So the reference's H.264 mode cannot be matched on this board as "the same
feature": the honest options are to keep MJPEG (what we do), or to add a
*software* transcoding path (ffmpeg is installed) as an explicitly different
pipeline, which needs a deliberate decision and its own gate.

## What actually limits the frame rate

The engine asks for whatever the settings file says (currently 60) and reports
`captured_fps: 10`. The bridge advertises 720p60 for MJPEG, so the 10 fps we
see is either the HDMI input signal itself or the practical USB2 throughput of
this dongle; `--drop-same-frames=30` also reduces the *streamed* rate on a
static desktop. Measuring which one it is needs the frame-timing probe with a
moving picture, and trying the vendor's 1080p default is the cheapest
experiment.

## Measured (device, 2026-09-19)

The capture resolution is now a reviewed setting like the frame rate: the owner
renders `BLI_KVM_USTREAMER_RESOLUTION` into the same environment file, the unit
reads `--resolution=${BLI_KVM_USTREAMER_RESOLUTION}`, and the interface's four
reviewed modes (1080, 720, 600, 480) map onto exactly those modes. Applied
through the API and read back from the engine:

```
apply resolution 1080 -> applied, read-back fps 60 quality 100
engine: resolution 1920x1080, desired_fps 60, captured_fps 10, 10, 9, 10
apply resolution 720  -> applied (restored)
```

Both 720p and 1080p capture at about 10 fps, and so does 640x480, so the limit
is not the output resolution. Moving the picture did not change it either: with
the pointer driven through the HID lease for twenty seconds (100 reports
applied), the capture stayed at 10 fps, and two `/snapshot` frames taken two
seconds apart differ (different digests and sizes), so the 10 fps is a live,
changing picture and not a static-desktop or `--drop-same-frames` artifact. The
bridge does not answer `VIDIOC_G_DV_TIMINGS`, so the input timing cannot be read
from the kernel; what remains is either the source's own output rate or the
dongle's real USB2 capture rate. Our settings are applied and read back, but
the delivered rate is lower than requested, which the interface should show.

The interface now shows it: the server reads the engine's read-only `/state`
socket (`crates/bli-kvm-server/src/video_engine.rs`, `BLI_KVM_USTREAMER_SOCKET`)
and returns `capturedFps` on `GET /api/vm/screen`; the screen menu renders
"Capture bridge delivers N fps" whenever the measured value differs from the
requested one, and omits the field entirely when the engine is unreachable
rather than repeating the requested rate.

The read-back path was verified on the device before deployment
(`evidence/private/video-state-readback-20260919-a.json`): the server user
`hardened-blikvm-server` is in the `hardened-blikvm` group, so it can traverse
`/run/hardened-blikvm-ustreamer` (0750) and read `ustreamer.sock` (0660); the
engine answered a `GET /state` as that user with `rc=0`. The exact ustreamer
6.11 shape is

```json
{"ok": true, "result": {"instance_id": "", "encoder": {"type": "HW", "quality": 0},
 "source": {"resolution": {"width": 1280, "height": 720}, "online": true,
            "desired_fps": 60, "captured_fps": 10},
 "stream": {"queued_fps": 0, "clients": 0, "clients_stat": {}}}}
```

so `captured_fps` and `desired_fps` live under `result.source`, and
`encoder.type = "HW"` with `quality: 0` is the second confirmation that the
camera-hardware MJPEG encoder ignores the quality setting.

## Consequences for our interface

- frame rate: applied and read back, keep it (the menu shows it);
- quality: hidden, it cannot change the picture on this board;
- video mode (mjpeg/h264), resolution, gop, frame detect: hidden, the pipeline
  has no H.264 and no resolution/GOP control of its own beyond the engine's
  start-up options;
- beyond the reference: if a viewer needs less bandwidth, the honest lever is a
  software transcode, which is a separate design decision.

## H.264 encode probe (device, 2026-09-19)

The operator asked whether the reference's H.264 mode can be matched. A
read-only probe of the exact board settled it:

- the only V4L2 nodes are `/dev/video0` (`cedrus`, the mainline **decoder**,
  driver 5.19.4, staging) and the USB2 capture device (`/dev/video1/2`); there
  is no encoder node;
- `ffmpeg -c:v h264_v4l2m2m` fails with *"Could not find a valid device"* — the
  SoC's hardware H.264 encoder block is real in the H616 datasheet, but no
  encoder is exposed by the installed stack;
- there is no Allwinner BSP userspace anywhere on the image: no `libcedarc`,
  `libmpp`, `libvenc`, `libMemAdapter` or `/dev/cedar_dev` / `/dev/mpp_service`;
- `ustreamer 6.11` only offers MJPEG encoders (`CPU`, `HW`, `M2M-VIDEO`,
  `M2M-IMAGE`); its H.264 sink (`--h264-sink` + `--h264-bitrate` / `--h264-gop`)
  and ffmpeg's `libx264` are **software** encoders;
- the vendor board confirms this: `main.yaml` runs
  `ustreamer --format=mjpeg --jpeg-sink=kvmd::ustreamer::jpeg` and has no H.264
  sink, and Janus is disabled/inactive.

So the reference's H.264/WebRTC path can only be a **software** transcode on
this board, delivered through a Janus/WebRTC stack we have not ported. Measured
cost on the four A53 cores (`ffmpeg -t 5 -preset veryfast -tune zerolatency`,
wall time for five seconds of 30 fps video):

| pipeline | frames | wall | realtime capability |
| --- | --- | --- | --- |
| synthetic 720p30 -> libx264 | 150 | 33.1 s | ~4.5 fps |
| synthetic 1080p30 -> libx264 | 150 | 72.5 s | ~2 fps |
| synthetic 720p30 -> libx264 ultrafast | 150 | 29.4 s | ~5 fps |
| **MJPEG 720p30 -> libx264 veryfast** | 150 | **10.7 s** | **~14 fps** |

The realistic MJPEG case is the important one: it fits the bridge's own 10 fps,
but there is still no H.264 delivery path. The honest position stays as
recorded: the video-mode switch, bitrate and GOP stay hidden; matching the
reference needs a ported WebRTC stack plus a deliberate software-transcode
decision (and a 1080p30 software encode is not viable at ~2 fps).
