# Work plan: sharp upscaling of the picture in the page (2026-10-08)

The board captures at most 1920x1080, and the operator's target drives a
2560x1440 monitor. The operator asked for the picture to be enlarged well:
drawn larger than 1920x1080, on the desktop and on phones, with more
sharpness than the browser's bilinear stretch, preferably by hardware. This
plan does it in the page, on the viewer's GPU. Nothing on the board changes;
the result ships as an application bundle.

Rules as in the earlier plans: English repository text, secrets only through
`project-memory-exec` (never in arguments, logs or receipts, and never a
program on its standard input under it), every change shipped as a signed
slot installed through the update API on the test board and verified there,
audit and design documents updated with evidence under `evidence/private`.
Additions for this plan:

- One commit per item. An item's own tests run before its commit; the full
  `scripts/check.sh` runs before the last commit of each phase and before
  every slot build. The branch is pushed after each phase.
- Every phase ends in a gate with numbers. A gate that is missed, or a time
  box that runs out, ends the plan there with the findings recorded.
- On the board: never take the HID lease, never send input to the target,
  never pulse ATX, never touch the operator's media. The operator's settings,
  switches, firewall mode, video settings and engine choice end as found.
- Third-party shader code comes only from pinned upstream sources checked by
  SHA-256, with its licence text shipped beside the web bundle and listed in
  the repository's third-party notes. Downloading it needs the operator's
  approval once.
- The upscaler runs only where the browser draws with a GPU: drawn in
  software, any filter pass on the picture cost Chromium a processor core at
  1920x1080 and 30 frames a second (`audit-20261007-beta3.md`).

## Where the work starts

- Release 0.1.0-beta.3 (slot 78, source `5753af5`; the board's system is that
  release's image).
- The capture (`hdmi-bridge-usb2.md`): a MacroSilicon USB 2.0 bridge
  (`345f:2131`) whose largest mode is 1920x1080; the H616 has no USB 3
  controller. The operator's target sends a 2560x1440 60 Hz signal (read once
  from the bridge's timing registers on 2026-10-08), which the bridge scales
  by 3/4. With a 1920x1080 desktop the target's GPU scales it up to that
  signal and the bridge back down: the picture is 1:1 in size, softened
  twice. Reading the bridge's vendor HID interface was researched and dropped
  by the operator: the live timing is only in registers that are risky to
  poll while capturing (ms-tools issue 14, the same chip).
- Not the board: the H616 has no memory-to-memory scaler a driver offers, and
  an enlarged frame would cost the encoder 1.8 times the work and the stream
  more bitrate for no information.
- The page (read on 2026-10-08):
  - `pages/desktop/screen/use-screen-fit.ts`: the fit scale is
    min(container / picture, 1), never above 100%; the zoom (menu 10 to 200%,
    pinch on touch) sets the element's size inside a scroll container; the
    display's `devicePixelRatio` is not taken into account, so the browser
    already stretches the picture on HiDPI screens at 100%. The effective
    enlargement is fit x zoom x devicePixelRatio.
  - Four ways to draw: MJPEG an `<img>` of the stream (`mjpeg.tsx`); WebRTC a
    `<video>` in two components (`vp8-webrtc.tsx` for VP8 and x264 through
    Janus, `h264-webrtc.tsx`); QUIC and Direct a canvas drawn by a worker with
    `drawImage` (`lib/quic/video.worker.ts`, `screen/direct.worker.ts`).
  - Pointer mapping (`mouse/absolute.tsx`, `getMediaSize`) takes the
    picture's size from `videoWidth`, `naturalWidth` or `canvas.width`: a
    canvas whose buffer has the screen's size would break it.
  - VP8's range correction is a CSS SVG filter (`lib/vp8-range.ts`), applied
    only where `drawsWithGpu()`; drawn through WebGL it must move into the
    shader.
  - The page uses no WebGL yet.

## Phase 0: measure and choose (host and the operator's devices)

1. Test images: 2560x1440 pages of text from 9 to 16 px, interface elements,
   code and one-pixel lines; their 1920x1080 versions made with box,
   bilinear and bicubic downscales and JPEG at the bridge's quality.
2. Real pairs (operator step): the operator shows the test image full screen
   on the target with a 2560x1440 desktop; one MJPEG frame is taken from the
   board, read only. It tells which downscale and sharpness the bridge
   applies.
3. Candidates as standalone WebGL2 shaders (scratch, not shipped):
   bilinear (the browser's), Catmull-Rom bicubic, Lanczos-2, bicubic with a
   sharpening tuned to the bridge's blur, Qualcomm SGSR 1 (one pass, made for
   phones, BSD-3-Clause), AMD FidelityFX FSR 1 (EASU and RCAS, MIT), and for
   desktop GPUs a small convolutional upscaler of the Anime4K family (MIT).
4. Quality against the 2560x1440 original at 4/3 and 2: PSNR, SSIM, the
   width of the 10-90 % rise across text edges, and the overshoot around
   text (halo), with crops in the audit.
5. Cost on real GPUs: a self-contained benchmark page the operator opens on
   an Android phone (Chrome), the desktop (Chrome or Edge) and, where
   available, Firefox and an iPhone: milliseconds per frame at 2560x1440 and
   at the screen's size, and frames lost over 30 s at 30 frames a second.

Gate 0: for desktop GPUs and for phone GPUs, an algorithm that narrows the
edge rise by at least 25 % against bilinear (or raises SSIM by at least
0.01), keeps the halo within 4 levels on text, and costs at most 1 ms
(desktop) or 2 ms (phone) per 2560x1440 frame. Where none passes on phones,
phones get bicubic or nothing. Time box: 8 hours.

Result (2026-10-08, `audit-20261008-upscaling-gate0.md`): passed for both
classes by a deringing pass (a 5x5 bilateral filter on the 1920x1080 frame)
followed by SGSR 1. The deringing pass was added after the real pair showed
that the bridge's JPEG ringing beside text kept every planned candidate over
the halo limit. Only SGSR's code (BSD-3-Clause) ships.

## Phase 1: the rendering core

1. `web/src/lib/upscale/`: a WebGL2 renderer that takes a `VideoFrame`, a
   `<video>`, an `ImageBitmap` or an `<img>`, and draws a rectangle of it at a
   given size with the chosen algorithm and sharpness; VP8's range table in
   the shader for VP8; textures reused, nothing allocated per frame;
   `webglcontextlost` and `webglcontextrestored` handled.
2. Pointer mapping independent of the drawing surface: every view states the
   picture's size (the resolution it already publishes), and `getMediaSize`
   uses it before the element's own size.
3. The effective enlargement (fit x zoom x devicePixelRatio) computed in one
   place and kept current on resize, zoom, orientation and pixel-ratio
   changes (`matchMedia`).
4. Tests: each shipped shader against a CPU reference of the same algorithm
   within one level (Playwright, SwiftShader); context loss and restore;
   pointer mapping at 100, 150 and 200 % with pixel ratios 1 and 2; licence
   texts present in the build.

Gate 1: the references agree, no allocation per frame, the pointer lands
where it did. Time box: 6 hours.

## Phase 2: QUIC

1. The video worker draws through the renderer in its OffscreenCanvas when
   upscaling is on and WebGL2 is there; otherwise `drawImage` as now.
2. The page tells the worker the displayed size and pixel ratio; the canvas
   buffer is the displayed size times the pixel ratio, at most 3840x2160
   (CSS stretches beyond).
3. The quality monitor shows the algorithm and its time per frame.
4. Contracts: `quic_video_contract.mjs` with upscaling on (the frame source's
   dark squares still at 16, edges sharper than bilinear by the gate's
   measure), pointer and touch mapping.

Gate 2: contracts green; on the board, on the operator's desktop and phone,
no frame lost to the upscaler at a 2560x1440 picture. Time box: 4 hours.

## Phase 3: WebRTC (H.264 and VP8)

1. A canvas `#screen` over a hidden `<video>`, frames taken with
   `requestVideoFrameCallback` (`requestAnimationFrame` where it is missing),
   in both WebRTC components.
2. VP8: the range table in the shader while upscaling; the SVG filter
   otherwise, as now.
3. Contracts: `vp8_range_contract.mjs` repeated with upscaling on, H.264's
   colour unchanged, pointer mapping.

Gate 3 as gate 2. Time box: 5 hours.

## Phase 4: MJPEG

1. Uploads from the `<img>` on animation frames while the tab is visible, or a
   reader of the stream (`fetch`, `createImageBitmap` per frame), chosen by
   the measured upload cost on the operator's phone.
2. Contracts as for the other views.

Gate 4 as gate 2. Time box: 4 hours.

## Phase 5: larger than 100 %, and phones

1. "Fill the window": with upscaling on, the fit may enlarge the picture
   beyond 100 %. "Target's size": the operator enters the target's resolution
   in the page (for example 2560x1440), and the preset shows the picture at
   that size (133 % here).
2. Pinch zoom: the buffer is kept during the gesture (CSS stretches) and drawn
   again at the final size once the gesture ends.
3. A hidden tab draws nothing; orientation and pixel-ratio changes take
   effect at once.
4. If the capped buffer costs a phone too much: draw only the visible region,
   a surface of the viewport's size over the scrolling `#screen`.

Gate 5, on the operator's Android phone: pinch zoom smooth, no frame lost,
the browser's processor use within 10 % of the same view without upscaling,
touches landing where tapped at 100 and 200 %. Time box: 6 hours.

Added at the operator's word (2026-10-10):

5. The quality monitor fits a phone: in Extended its window is as wide as
   the screen allows and as tall as its rows, every row in view, and no
   label or value is cut short; one that does not fit goes on a second
   line.
6. The monitor shows how busy the page's own threads are (the main thread
   and the picture's worker): the page cannot read the browser's processor
   use, and gate 5 compares this with upscaling on and off.
7. The phone's cursor: the tip of the arrow the page draws is where the
   target's cursor is, not the corner of the arrow's box.

## Phase 6: settings and safeguards

1. Screen menu: Upscaling Off or Auto, sharpness low, medium or high, kept
   per browser. Auto enlarges only when the effective enlargement is above
   1.1 and the browser draws with a GPU. Off by default in this release; the
   operator decides at this gate whether Auto becomes the default.
2. A watchdog: when frames drawn fall below 90 % of frames received for 5 s
   with upscaling on, upscaling switches off for the session, with a message
   and a quality-monitor row.
3. Drawn in software, no WebGL2, or a lost context: the plain path, silently.
4. English and Russian texts.
5. Contracts: the menu and its memory, Auto's conditions, the watchdog (a
   slowed draw injected), and no upscaling in a browser that draws in
   software.

Time box: 4 hours.

## Phase 7: release

1. `scripts/check.sh`, a signed slot, installed on the test board; the
   release probes (GUI walks at a desktop's and a phone's size, the quality
   monitor open, WebRTC H.264 and VP8, the QUIC picture, the phone's reach,
   the engine menu), and the picture of each mode against the MJPEG frame
   with upscaling on at 133 and 200 %.
2. The operator's devices: Android Chrome and the desktop (Chrome or Edge);
   Firefox if the operator approves installing it on the host; an iPhone if
   there is one.
3. `docs/design/screen-upscaling-v1.md`, the audit note, the README's
   features.
4. Application release 0.1.0-beta.4 (the bundle, SHA256SUMS, notes, a pointer
   in beta.3's notes), only when the operator says to publish.

Added at the operator's word (2026-10-10), before the release's slot:

5. The board's display goes dark after a time without activity, as in
   Hardened NanoKVM: Settings > Device offers never (the default, as now),
   15 or 30 seconds, 1, 3, 5, 10 or 30 minutes, or an hour. Dark means the
   ST7789's backlight off and nothing drawn; the board's button and a new
   choice light it again (not a change on the status screen, which changes
   every few seconds). The display daemon
   (`bli-kvm-boardd`, in the application slot) keeps the time; the board's
   owner keeps the choice across restarts; the board's status that the
   update runtime reads for its health check keeps its shape. Contracts:
   the setting's API and page in both languages, the daemon's timer, the
   button and the redraws while dark. On the board: the operator sees it
   go dark and the button light it.

Time box: 4 hours plus the operator's checks.

## Operator steps

- Approve downloading the upstream shader sources (once).
- Show the test image on the target with a 2560x1440 desktop (phase 0, item
  2) when asked.
- Open the benchmark page on the phone and the desktop (phase 0, item 5), and
  check the slots on them at gates 2 to 5.
- Agree to slot installs, decide the default at gate 6, and say when to
  publish.
