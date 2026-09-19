# Vendor V4 video-settings source map (operator-supplied)

This baseline records where the *vendor* BliKVM V4 video settings live, so our
parity work can check that every setting we expose is really attached and that
every setting we hide has a reason. The map comes from the operator's
`blikvm_v4_video_settings_source_map_for_agent.md` (2026-09-19) and is about the
upstream repositories `blikvm-web-client`, `blikvm-web-server` and `blikvm`; it
contains no board addresses or credentials.

## Vendor wiring

| Setting | Frontend | Backend | Runtime effect |
| --- | --- | --- | --- |
| video enable/disable | `SettingsVideo.vue` | `video.route.js` `POST /api/video?action=start|stop` | start/stop the ustreamer service |
| video mode (h264/mjpeg) | `SettingsVideo.vue` `handleVideoModeChange` | `video.js` `_param` | `mjpeg` -> `initMjpeg`, `h264` -> Janus `initVideo` |
| resolution | `SettingsVideo.vue` `resolutionOptions` (9 entries) | `apiResolutionChange()` (MangoPi only) -> `video.setResolution()` | `--resolution=` + service restart |
| desired FPS | `SettingsVideo.vue` slider 10..60 step 10 | `POST /video/config?action=set` | `--desired-fps=` |
| MJPEG quality | slider 10..100 step 10, but only shown for 4B/CM4 | `video.js` `quality` | passed to `kvmd-video.sh`; for V4 the source map says to verify whether it affects the MS2131 at all |
| H.264 bitrate | slider 0.1..10 Mbps -> kbps | `POST /video/config?action=set` | `--h264-bitrate=` |
| H.264 GOP | slider 0..60, default 30 | `POST /video/config?action=set` | `--h264-gop=` |
| live state | `SettingsVideo.vue` | `apiGetVideoState()` / `wsGetVideoState()` | reads `source.resolution.width/height`, `source.captured_fps`, `stream.queued_fps` |

Resolution list the vendor UI offers (frontend-only, not a hardware
capability list): `1920x1080`, `1600x1200`, `1360x768`, `1280x1024`,
`1280x960`, `1280x720`, `800x600`, `720x480`, `640x480`.

Vendor defaults (`app_default_config.js`): port `10004`, fps `30`, quality `80`,
kbps `2000`, gop `30`, resolution `1920x1080`.

## The vendor V4 H.264 path is software

`lib/kvmd-video.sh` detects `MangoPi Mcore` and runs, for V4:

```
ustreamer --format=MJPEG --encoder=LIBX264-VIDEO --device=<jpeg device>
          --resolution=$resolution --drop-same-frames=30
          --desired-fps=$fps --h264-bitrate=$kbps --h264-gop=$gop
          --h264-sink=demo::ustreamer::h264
```

So H.264 is `libx264` on the CPU into a shared-memory sink that Janus/WebRTC
consumes; the H616 hardware video engine is not used (see
`docs/baselines/hdmi-bridge-usb2.md`). The source map also notes the observed
log `H264 Encoder libx264: Input format is RGB24, converting to I420`, i.e. an
extra RGB24->I420 step that a JPEG->YUV420 path could avoid. The fallback path
(when no JPEG device is found) hardcodes `1920x1080` and does not enable H.264.

The vendor frontend shows the H.264 option only for `4B`/`CM4` even though the
backend has a V4 H.264 path, so the vendor UI and backend disagree for V4.

## What this means for our stack

- Our exposed settings are `fps` and `resolution`; the wiring and its device
  proof are recorded in `docs/baselines/hdmi-bridge-usb2.md` and the P1.1
  section of `docs/design/parity-work-plan-v2.md`.
- `quality` is hidden because the engine runs the camera-hardware MJPEG encoder
  (`result.encoder.type=HW`, `quality: 0`), so ustreamer cannot change the JPEG
  quality; this matches the vendor only showing the slider for 4B/CM4.
- video mode, H.264 bitrate, GOP and frame detect stay hidden until a
  Janus/WebRTC delivery path exists; the software encode budget is measured in
  `docs/baselines/hdmi-bridge-usb2.md`.
- The operator's recommended matrix (resolution x requested FPS with
  `captured_fps`/`queued_fps` and bandwidth) is the device check added to P1.1.

## Our wiring (host-verified, 2026-09-19)

| Setting | UI | HTTP | Server | Owner | On disk | Unit argument | Engine field | Read-back |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| fps | `web/src/pages/desktop/menu/screen/fps.tsx` | `POST /api/vm/screen {"type":"fps"}` | `crates/bli-kvm-server/src/video_settings.rs` | `crates/bli-kvm-control-broker/src/video_settings.rs` | `/var/lib/hardened-blikvm-video/ustreamer.env` `BLI_KVM_USTREAMER_FPS` | `--desired-fps=${BLI_KVM_USTREAMER_FPS}` | `result.source.desired_fps` | `GET /api/vm/screen` `fps` |
| resolution | `web/src/pages/desktop/menu/screen/resolution.tsx` | `POST /api/vm/screen {"type":"resolution"}` | same | same | same `BLI_KVM_USTREAMER_RESOLUTION` | `--resolution=${BLI_KVM_USTREAMER_RESOLUTION}` | `result.source.resolution` | engine only (the server response has no resolution field) |
| captured state | (display) `fps.tsx` | `GET /api/vm/screen` | `crates/bli-kvm-server/src/video_engine.rs` | — | — | — | `result.source.captured_fps`, `result.source.resolution`, `result.stream.queued_fps` | `capturedFps`, `width`, `height`, `queuedFps` |
| quality | hidden | `{"type":"quality"}` accepted but not advertised | same | same `BLI_KVM_USTREAMER_QUALITY` | `--quality=${BLI_KVM_USTREAMER_QUALITY}` | `result.encoder.quality` (always `0`, `type=HW`) | — |
| video mode / bitrate / GOP / frame detect | hidden | `unsupported_setting` | — | — | — | — | — | — |

The capability advertises exactly `fps` and `resolution`
(`crates/bli-kvm-server/src/lib.rs`, `VideoSetting::Fps`,
`VideoSetting::Resolution`). Quality is hidden because the camera-hardware
encoder ignores it; the remaining entries are hidden because there is no H.264
delivery path.

### The matrix this check must produce

| Resolution | Requested FPS | captured_fps | queued_fps | MJPEG bandwidth | Notes |
| --- | --- | --- | --- | --- | --- |
| 1920x1080 | 10..60 | (device) | (device) | (device) | |
| 1280x720 | 10..60 | (device) | (device) | (device) | |
| 800x600 | 10..60 | (device) | (device) | (device) | |
| 640x480 | 10..60 | (device) | (device) | (device) | |

The device sweep fills this in; nothing in this table may be inferred from the
vendor UI list.

## Device check of the retained vendor files (2026-09-19)

The operator's map describes the newer `blikvm-web-server` generation
(`/api/video/config`, `SettingsVideo.vue`, `useVideoResolution.js`). The files
retained on this exact board are an older vendor revision, and they change one
important conclusion:

- `/opt/bin/blikvm/package/ustreamer/kvmd-video.sh` has two branches. The
  **Raspberry Pi 4B / CM4** branch uses
  `--device=/dev/video0 --dv-timings --format=uyvy --encoder=omx --h264-sink=demo::ustreamer::h264`.
  The **V4/H616** branch (`Mango Pi Mcore`) runs only:

  ```
  ustreamer.bin --format=MJPEG --device=<first JPEG device> \
    --resolution=1920x1080 --host=0.0.0.0 --port=8008 \
    --drop-same-frames=30 --desired-fps=20
  ```

  with a fallback `--device=/dev/video1 --resolution=1920x1080` and no
  `--desired-fps`.
- So `--encoder=LIBX264-VIDEO`, `--h264-bitrate`, `--h264-gop` and the
  `demo::ustreamer::h264` sink in the map's section 6.2 belong to the **Pi**
  path, not to the V4 path on this image. The vendor map's V4 H.264 claim is
  therefore not evidence of a V4 H.264 path here; it matches the newer
  `blikvm-web-server` build (the reference stand), which this board does not
  ship.
- The V4 vendor defaults visible here are `1920x1080` and `desired-fps=20`; the
  newer build's `app_default_config.js` (`fps 30`, `quality 80`, `kbps 2000`,
  `gop 30`, `1920x1080`) is not present on the board (`app_default_config.js`
  was not found).
- Our running engine matches the reviewed subset:
  `/usr/bin/ustreamer --device=/dev/kvmd-video --persistent --format=mjpeg
  --resolution=1280x720 --desired-fps=60 --quality=100 --drop-same-frames=30
  --unix=/run/hardened-blikvm-ustreamer/ustreamer.sock --unix-rm --unix-mode=0660`,
  i.e. MJPEG only, no H.264.

Consequence: the physical board has no vendor H.264 path at all (neither
hardware nor `libx264`); the map's H.264 pipeline describes the reference/newer
vendor generation. The H.264 decision in
`docs/baselines/hdmi-bridge-usb2.md` is unchanged and now has a second,
independent confirmation from the retained vendor script.
