# Sharp upscaling of the screen in the page

Plan: `docs/baselines/work-plan-20261008-screen-upscaling.md`. This note
fixes what the plan leaves to its design: the algorithm and its passes, the
renderer's contract, how the pointer and the enlargement are worked out,
how each view draws through it, the settings and the safeguards. The record
of what was measured follows the design as the work goes on.

Status (2026-10-09): phases 2 to 5 built (phase 5 without its fourth
item), gates 2 to 5 to be checked on the operator's devices. Gate 0 chose
the algorithm (`docs/baselines/audit-20261008-upscaling-gate0.md`); the
renderer, the pointer mapping and the enlargement are built and tested on
the host; the QUIC, WebRTC and MJPEG views draw through the renderer, the
setting is in the Screen menu with the picture's size, and the watchdog
stops upscaling where frames are lost to it. The operator's phone found
the QUIC picture standing still with upscaling on; the causes and their
fixes, and the gates' records, are in
`docs/baselines/audit-20261009-upscaling-phases.md`.

## The algorithm

The board captures at most 1920x1080; the operator's target drives a
2560x1440 display, which the capture bridge reduces by 3/4 with a filter
close to a triangle and sends as JPEG of IJG quality 75, 4:2:2. The page
enlarges that frame on the viewer's GPU in two passes:

1. Deringing, at the frame's size: a 5x5 bilateral filter, its spatial
   weight a Gaussian of sigma 1.5 pixels, its range weight a Gaussian of the
   luma difference with sigma 12, 16 or 20 levels. The JPEG's ringing beside
   text (a few levels) is smoothed, the text's own edges (a hundred levels
   and more) are kept. Without it every filter sharper than bilinear keeps
   that ringing, and gate 0 was missed by all of them.
2. SGSR 1 (Qualcomm's Snapdragon Game Super Resolution, one pass, made for
   phones, BSD-3-Clause) from the deringed frame to the drawn size, with an
   edge sharpness of 1.5 or 2.0.

| Sharpness | Deringing sigma | SGSR edge sharpness |
| --- | --- | --- |
| low | 20 levels | 1.5 |
| medium (gate 0's choice) | 16 levels | 2.0 |
| high | 12 levels | 2.0 |

Two changes to SGSR keep its output the same on every GPU. Its bilinear
sample is computed from the four nearest texels, which its edge test reads
anyway, instead of by the GPU's filter, whose precision differs. Its edge
threshold is 8.37 levels instead of 8: with the sample between its two
neighbours the vote is exactly twice their difference, so a difference of
4 levels tied with 8, and each GPU (and the CPU) broke the tie its own way,
up to 23 levels apart on the same pixel. A frame enlarged by 4/3 or 2 gives
votes in 4096ths of a level, none within 1e-4 of a level of 8.37.

## The renderer

`web/src/lib/upscale/upscaler.ts` (`Upscaler`), with the programs in
`shaders.ts`:

- `Upscaler.on(canvas, { onLost, onRestored })` makes a WebGL2 context on a
  canvas or an OffscreenCanvas (no alpha, no antialiasing, no depth), or
  returns null where the browser has no WebGL2.
- `draw(frame, width, height, { mode, vp8Range, crop })` draws a frame of
  `width` x `height` pixels over the whole canvas, the canvas's drawing
  buffer being the drawn size. The frame is anything WebGL uploads: a
  `VideoFrame`, a `<video>`, an `<img>`, an `ImageBitmap` or a canvas.
  `mode` is `plain` (the GPU's bilinear filter) or a sharpness; `crop` is
  the part of the frame drawn, in frame pixels (all of it by default);
  `vp8Range` applies the table of `web/src/lib/vp8-range.ts` before
  anything else, so that the board's VP8 picture comes out as its JPEG.
  It returns false when nothing was drawn.
- The frame's texture and the deringed frame (a texture and its
  framebuffer of the frame's size) are made once and reused; a frame of
  another size reallocates them once. Drawing a frame of an unchanged size
  makes no texture, framebuffer, program, shader, buffer or vertex array.
- A lost context: the lost event is taken (so that the browser may restore
  it), the renderer drops its objects, `onLost` is called and `draw`
  returns false. When the browser restores the context the renderer makes
  its objects again and calls `onRestored`.
- Rows: the frame's row 0 is its top row in every texture; the last pass
  turns `gl_FragCoord` upside down for the canvas, whose rows count from
  the bottom.

The SGSR licence ships beside the built page as `THIRD-PARTY-NOTICES.txt`
(`web/public/`), and `scripts/check.sh` checks the build for it
(`docs/licensing.md`).

## Where the pointer falls

The screen element is laid out at the picture's shape and the picture drawn
into it. What the element holds need not have the picture's size: an
upscaled canvas has the drawn size times the pixel ratio, capped. So every
view states the picture's size on its `#screen` element,
`data-picture-width` and `data-picture-height`, from the resolution it
publishes (`web/src/lib/pointer-map.ts`, `pictureAttributes`), and the
absolute mouse maps the pointer by that size before the video's, the
image's or the canvas's own (`pictureSize`, `pointInPicture`). Touch moves
the cursor by the finger's movement over the element's extent and needs no
picture size.

## How much the picture is enlarged

`use-screen-fit.ts` works out, in one place, the picture's drawn size in
CSS pixels (the fit in the view times the zoom of the menu or a pinch),
the display's pixel ratio, and the enlargement: fit x zoom x pixel ratio.
The fit is at most 100 % for a plain picture; an upscaled one fills the
view (Fill) or is laid out at the target's resolution (`fitScale`,
`fitFor`; "The setting"). Above 1 the browser enlarges the picture; at a 100 % fit on a
display of ratio 2 it already does. The fit follows resizes and
orientation changes (a resize observer), the pixel ratio a media query on
the current ratio (`usePixelRatio`), which a browser's zoom or a move to
another display changes. The scroll view carries
`data-picture-enlargement` and `data-picture-pixel-ratio` for probes.

The drawn size changes with every move of a pinch's fingers (the touch
zoom goes to 400 %, the menu's to 200 %). The hook also gives it once it
has not changed for 150 ms (`settled`), and the views size a canvas's
buffer by that: while the fingers move the browser stretches the canvas as
it is, and the picture is drawn at its new size once they stop. The
canvas's CSS size follows the fingers.

## The setting

`web/src/lib/upscale/settings.ts`: Off or Auto and a sharpness (low, medium
or high), kept by the browser (`hardened-blikvm-upscale` in its local
storage), Off by default. The Screen menu's Upscaling entry sets it
(`menu/screen/upscale.tsx`), and says when the browser draws without a GPU
and why upscaling stopped on the page. Upscaling runs where the setting is
Auto, the browser draws with a GPU (`drawsWithGpu`, the renderer WebGL
names is not a software rasterizer) and nothing has stopped it on this
page load: no WebGL2 where the view draws, its context lost, or frames lost
to it (the watchdog, below; `upscaleStoppedAtom`); the view then goes on
plainly. A frame shown more
than 1.1 times its size (the enlargement above) is drawn with the
sharpness into a buffer of the shown device pixels, at most 3840x2160 and
of the picture's shape (`drawing`, `bufferSize`); otherwise at its own
size, plainly, the browser scaling the canvas.

The size of an upscaled picture: Fill (the default) lays it out as large
as the view lets it, beyond its own size; the target's size lays it out at
a resolution the operator types (2560x1440 shows a 1920x1080 capture at
133 %, each of the target's pixels on one of the display's at a pixel
ratio of 1). The fields take their keys themselves: while one has the
focus the page sends no key to the target, as for its other fields.

## The views

- QUIC (`screen/h264-quic.tsx`, `lib/quic/video.worker.ts`): the worker is
  handed a canvas for WebGL2 when upscaling, for a 2D context otherwise (a
  canvas has one kind of context, so the view mounts a new canvas and
  worker when upscaling starts or stops). The page tells the worker where
  the picture is shown (its CSS size and the pixel ratio) and the
  sharpness; the worker reports how it draws, its time per frame (the
  GPU's, where the browser offers timer queries) and the canvas's buffer.
  The quality monitor's Extended view has an Upscaling row:
  `SGSR, medium, 0.27 ms GPU` (SGSR naming the deringing pass and SGSR),
  `on, not enlarged`, `off`, or why it stopped (`stopped: frames lost`).
  The new canvas's worker unwatches the picture and watches it again at
  once; the session passes over the "stopped" with which the gateway
  answers the unwatching (`docs/design/quic-transport-v1.md`, "Messages"),
  which once came after the new subscription and sent H.264 to WebRTC.

- WebRTC, H.264 and VP8 (`screen/h264-webrtc.tsx`, `screen/vp8-webrtc.tsx`,
  `screen/upscaled-video.tsx`): plainly the video element is the screen.
  Upscaled, the video plays out of sight (a pixel, transparent, so that
  the browser still presents its frames) and a canvas is the screen; each
  frame the video presents (`requestVideoFrameCallback`) is drawn through
  the upscaler on the page's thread, and drawn again when the picture is
  shown at another size. VP8 goes through its range table in the shader
  instead of the SVG filter.
- MJPEG (`screen/mjpeg.tsx`, `screen/upscaled-mjpeg.tsx`,
  `lib/mjpeg-parts.ts`): plainly an image is the screen. Upscaled, the page
  reads the stream with `fetch` and cuts it into its parts (by
  `Content-Length`, or at the next boundary), decodes each off the page's
  thread (`createImageBitmap`), keeps the newest and draws it on the next
  animation frame; the last frame is drawn again when the picture is shown
  at another size. A hidden page decodes nothing.
- Input binds to the element the view mounts as the screen
  (`screenElementAtom`, `useScreenRef`), so that a canvas replacing a video
  or a new canvas for a new worker takes the mouse and touch at once.
- A hidden page draws nothing. The QUIC worker is told and decodes its
  frames without drawing them, the newest drawn as soon as the page is
  seen; the WebRTC and MJPEG views get no frame callback or animation
  frame. The QUIC view's rule that gives the picture up to WebRTC after
  ten seconds without a frame drawn counts no time while the page is
  hidden, nor a gap of more than three seconds between the worker's
  reports (a phone's screen off).

## The safeguards

- No GPU (`drawsWithGpu`), no WebGL2 where the view draws, or a lost
  context: the plain path, without a notice; the Screen menu and the
  monitor's row say why.
- The watchdog (`web/src/lib/upscale/watchdog.ts`): while a view draws
  through the upscaler it counts the frames it received and the frames it
  drew; when fewer than 90 % of the frames of the last five seconds were
  drawn, upscaling stops for the page load, the page says so once in a
  notice ("Upscaling switched off"), and the view goes on plainly. It does
  not judge before five seconds were counted, nor on fewer than 15 frames
  (a still screen sends one every two seconds). A view counts as received
  only what it could have drawn: the QUIC worker the frames it hands to the
  decoder, so that frames the network lost (never complete, or after a
  break in the chain) do not count against the upscaler; a new session
  starts the count over. The WebRTC and MJPEG views draw on the page's
  thread, where its own clock cannot tell a frame lost from one that came
  within a frame of the display (a draw that holds the thread up holds up
  everything on it, and the frames then come together): WebRTC counts the
  frames the video presented (`presentedFrames`, kept by the compositor),
  MJPEG the frames the stream sent, at most one in 20 ms by their capture
  time (`X-Timestamp`, as the board's streamer says it). A page seen again
  starts the count over. On the host a draw slowed by 240 ms a frame stops
  it within 1 to 9 s in each view, and a QUIC path losing 3 % of its
  datagrams for 8 s leaves it on.

## Tests

- `tools/playwright/upscale_renderer_contract.mjs`: in Chromium drawing with
  SwiftShader, a frame of the synthetic picture with the JPEG's ringing is
  drawn in every mode, with and without VP8's table, enlarged by 4/3, by 2
  and by an uneven ratio, a part of it, made smaller, and given as an
  `ImageBitmap`, an `<img>` and a `VideoFrame`; every channel within one
  level of a CPU reference of the same passes
  (`tools/playwright/upscale_reference.mjs`). Frames of an unchanged size
  make nothing; a lost context draws nothing, and once restored draws as
  before.
- `tools/test_pointer_map.mjs`: the pointer on the same picture pixel at
  100, 150 and 200 % with pixel ratios 1 and 2, and with a capped buffer of
  another shape.
- `tools/playwright/screen_enlargement_contract.mjs`: the enlargement
  reported against the view's geometry, through a pixel-ratio change, a
  resize and the zoom; upscaled, filling a view larger than the picture
  and laid out at a 2560x1440 target, the size set in the Screen menu with
  no key sent on to the target.
- `tools/test_upscale_settings.mjs`: the setting as the browser keeps it,
  the buffer's size, and when a frame is enlarged.
- `tools/test_upscale_watchdog.mjs`: the watchdog's rule: every frame drawn,
  one in ten lost, one in five drawn, a decoder that falls behind, the first
  five seconds, a still screen, and a new session.
- `tools/test_quality_monitor.mjs`: the Upscaling row in each case,
  stopped included.
- `tools/test_mjpeg_parts.mjs`: the parts of an MJPEG stream by their
  length and by the next boundary, in chunks of any size, with their
  capture times, and what is not a stream.
- `tools/playwright/vp8_range_contract.mjs`: upscaled at a pixel ratio of 2,
  VP8's greys within 1.5 levels of the JPEG and its colours within 4, H.264
  within 1.5, the buffer of the shown device pixels, no SVG filter, the
  mouse where it is; a slowed draw stops upscaling and the video plays.
- `tools/playwright/mjpeg_upscale_contract.mjs`: the stream's patches
  within 2 levels of the plain image, the squares' edges rising at least
  15 % more narrowly than the browser's own scaling of it, the mouse where
  it is; with a stream that goes on, a slowed draw stops upscaling and the
  image is the screen again.
- `tools/playwright/quic_video_contract.mjs`: the QUIC picture at 200 %,
  plain and upscaled: the upscaled canvas has the shown size, the frame's
  number reads, the dark squares stay at 16, the squares' edges rise at
  least a quarter more narrowly than the browser's stretch of the plain
  canvas, nothing is dropped, and the mouse and a finger land where they
  are on the picture. Upscaling switched off, on and off in the Screen menu
  on a live page, the picture stays on the session and moves on; a lossy
  path leaves upscaling on, and a draw slowed in the worker stops it with
  the notice while the picture goes on plainly. On a phone a pinch keeps
  the buffer while the fingers move and draws at the new size once they
  stop; a page hidden for 12 s draws nothing and keeps the picture on the
  session.

## Measured

Gate 0 (host and the operator's devices): `audit-20261008-upscaling-gate0.md`.
On the real capture, deringing 16 + SGSR: SSIM 0.938 against bilinear's
0.906, the edges' rise 1.78 against 2.34 px, the halo 1 level above
bilinear's; 0.27 ms per 2560x1440 frame on an RTX 3080 Ti, 1.09 ms on an
Adreno 830, no frame lost over 30 s at 30 frames a second on either.
