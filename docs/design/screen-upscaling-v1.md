# Sharp upscaling of the screen in the page

Plan: `docs/baselines/work-plan-20261008-screen-upscaling.md`. This note
fixes what the plan leaves to its design: the algorithm and its passes, the
renderer's contract, how the pointer and the enlargement are worked out,
how each view draws through it, the settings and the safeguards. The record
of what was measured follows the design as the work goes on.

Status (2026-10-08): phase 1 done. Gate 0 chose the algorithm
(`docs/baselines/audit-20261008-upscaling-gate0.md`); the renderer, the
pointer mapping and the enlargement are built and tested on the host; no
view draws through the renderer yet.

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
CSS pixels (the fit in the view, at most 100 %, times the zoom of the menu
or a pinch), the display's pixel ratio, and the enlargement: fit x zoom x
pixel ratio. Above 1 the browser enlarges the picture; at a 100 % fit on a
display of ratio 2 it already does. The fit follows resizes and
orientation changes (a resize observer), the pixel ratio a media query on
the current ratio (`usePixelRatio`), which a browser's zoom or a move to
another display changes. The scroll view carries
`data-picture-enlargement` and `data-picture-pixel-ratio` for probes.

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
  resize and the zoom.

## Measured

Gate 0 (host and the operator's devices): `audit-20261008-upscaling-gate0.md`.
On the real capture, deringing 16 + SGSR: SSIM 0.938 against bilinear's
0.906, the edges' rise 1.78 against 2.34 px, the halo 1 level above
bilinear's; 0.27 ms per 2560x1440 frame on an RTX 3080 Ti, 1.09 ms on an
Adreno 830, no frame lost over 30 s at 30 frames a second on either.
