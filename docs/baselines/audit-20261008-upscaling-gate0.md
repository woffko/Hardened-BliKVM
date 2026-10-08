# Screen upscaling, gate 0: the bridge's JPEG ringing, and SGSR after a deringing pass (2026-10-08)

Phase 0 of `work-plan-20261008-screen-upscaling.md`: measure the candidate
upscalers on the host and on the operator's devices, and choose one for
desktop GPUs and one for phone GPUs. The gate passed: both classes get the
same two passes, a 5x5 bilateral deringing pass on the 1920x1080 frame and
then Qualcomm's SGSR 1 to the output size. The deringing pass was not among
the plan's candidates; it was added after the real capture showed that no
planned candidate kept the halo within the limit (below). Phase 0 ran from
16:30 to about 18:45 UTC, within its 8 hours. Receipts, crops and the
device runs are under `evidence/private/upscale-20261008` (not in Git).

## The bench

`tools/upscale/` (commits `c0de480` to `8ebba72`; lab tools, never shipped):

- `patterns.js`: a 2560x1440 picture of text from 9 to 16 px (sans-serif and
  monospace, dark on light and light on dark), interface elements, one-pixel
  lines at 2 to 5 px spacing and a checkerboard, 72 pixel-aligned step edges at
  every phase of the 3/4 grid, and a gradient with colour bars.
- `kernels.js`: the downscales a bridge may make (area, bilinear, bicubic, and
  a triangle and Catmull-Rom stretched by the reduction), with a movable
  sampling grid.
- `candidates.js`: the candidates as WebGL2 programs: bilinear, Catmull-Rom,
  Lanczos-2, sharpening before bicubic, SGSR 1, FSR 1 EASU and RCAS, the
  Anime4K CNN x2 S and M (translated from the pinned mpv shaders), and the
  deringing pass. Licences, upstream commits and SHA-256 of the ported and
  loaded files: `tools/upscale/THIRD-PARTY-NOTICES.txt`.
- `metrics.js`, `bench.js`, `run_quality.mjs`: every source is enlarged to
  2560x1440 by every candidate in headless Chromium (SwiftShader) and measured
  against the original; for a captured frame against the original as the
  capture chain delivered it. Unit tests: `tools/test_upscale_bench.mjs`.
- `device-page.template.html`, `build_device_page.mjs`: a self-contained page
  the operator opened on the phone and the desktop: the cost of each candidate,
  30 s of a 30 frames a second feed drawn by the finalists, and the
  calibration picture shown full screen for the real pair.

The measures, all on BT.601 luma:

- SSIM (8x8 windows every 4 px) and PSNR per panel; the tables give the mean
  SSIM of the two text panels.
- Rise: the median over the 72 step edges of the 10-90 % rise of the profile
  across the edge, in the original's pixels (a perfect step measures 0.8).
- Halo: the background next to text (within 2 px in the original) moved away
  from the text, lighter around dark text and darker around light text, 95th
  percentile, the larger of the light-on-dark text panel's and the interface
  panel's. "Halo added" is a candidate's minus bilinear's on the same source:
  bilinear never leaves its source's range, so its halo is the source's own.
  The first halo measure (the output beyond the original's local 5x5 range)
  also counted strokes made darker than the original's anti-aliased ones,
  which is contrast regained, not halo; it was replaced before the gate was
  read and is kept in the receipts as `halo`.

Gate 0, from the plan: SSIM at least 0.01 above bilinear or the rise at least
25 % narrower; halo added at most 4 levels; at most 1 ms (desktop) or 2 ms
(phone) per 2560x1440 frame.

## The real pair

The target is the operator's desktop itself (2560x1440 at 100 %). The
operator opened the device page there and showed the calibration picture (the
synthetic picture as a PNG, drawn at one picture pixel per device pixel) full
screen while the board's MJPEG stream was read for 3 minutes, a frame kept
every 2 s, read only (`grab` receipt in the evidence). The frames that show
the picture are byte-identical. What they show:

- Levels: the frame is the original mapped by 1.141 (v - 9.97) (rms 3.2
  levels over 101783 flat pixels): blacks under about 10 and whites over about
  233 clip. That is the operator's capture chain (the target's output range or
  the bridge's input), not the upscaler; it is reported, not changed here.
- Downscale: closest to the triangle filter stretched by the reduction (30.9
  dB on the text, line and edge panels after the level mapping), then the
  stretched Catmull-Rom (29.4), area (29.0), bilinear (26.8) and bicubic
  (25.0); the sampling grid sits (-0.75, -1) source pixels off. The frame's
  edge rise is 2.03 px against the area downscale's 1.96. The bridge's JPEG is
  IJG quality 75 with 4:2:2 chroma (read from the frames earlier the same
  day).
- The ringing of that JPEG beside text: bilinear averages it away, and every
  sharper candidate keeps it. On the real frame no planned candidate kept the
  halo added within 4 levels (table). A deringing pass before the upscaler (a
  5x5 bilateral filter, range weight from the luma difference with sigma 8,
  12, 16 or 20 levels) smooths the ringing, a few levels, and keeps the text's
  own edges, a hundred levels and more.

## Quality

The real frame (against the original as delivered: levels mapped, moved one
pixel right and down) and the synthetic source closest to the bridge (the
stretched triangle, then JPEG at 0.75 through Chromium's encoder, 4:2:0):

| Candidate | Real: SSIM (vs bilinear) | Rise | Halo added | Triangle+JPEG: SSIM | Rise | Halo added |
| --- | --- | --- | --- | --- | --- | --- |
| bilinear | 0.906 | 2.34 | 0 | 0.906 | 2.31 | 0 |
| bicubic | +0.009 | -7 % | +6 | +0.010 | -6 % | +5 |
| SGSR 1 | +0.012 | -25 % | +11 | +0.019 | -23 % | +9 |
| FSR 1 EASU | +0.010 | -9 % | +6 | +0.014 | -9 % | +4 |
| Anime4K M | +0.025 | -34 % | +7 | +0.028 | -32 % | +6 |
| deringing 16 + FSR EASU | +0.022 | -9 % | -1 | +0.031 | -9 % | -2 |
| deringing 16 + SGSR 1 | +0.032 | -24 % | +1 | +0.044 | -23 % | -1 |

Over all eight sources (the real frame; area, triangle, bilinear and bicubic
downscales, with and without JPEG; and twice the enlargement, a 1280x720 area
downscale with and without JPEG): deringing 16 + SGSR passes on all eight
(SSIM +0.029 to +0.044, halo added -1 to +2), as do deringing 16 + FSR EASU
and deringing 20 + SGSR. Deringing 12 + SGSR misses the halo on two (+5 on the
bicubic downscale and on twice the enlargement); plain SGSR and plain FSR EASU
pass only without JPEG, and Anime4K nowhere. Crops: `real-pair` and `tuning`
crop sheets in the evidence.

## Cost

The device page's runs (milliseconds per 2560x1440 frame, the 1920x1080 frame
already on the GPU; the deringing pass measured with sigma 12, the same work
as 16):

| Device | Bilinear | SGSR | FSR EASU | Deringing + SGSR | Deringing + EASU | Anime4K S / M |
| --- | --- | --- | --- | --- | --- | --- |
| Phone: Adreno 830, Chrome 151 | 0.41 | 0.35 | 1.10 | 1.09 | 1.81 | 6.0 / 9.0 |
| Desktop: RTX 3080 Ti, Edge (GPU timer) | 0.16 | 0.20 | 0.51 | 0.27 | 0.33 | 0.61 / 0.88 |

Thirty seconds of a 30 frames a second feed at the screen's size (2672x1203
on the phone, 2560x1440 on the desktop) lost no frame with bilinear, SGSR,
FSR EASU, deringing + SGSR or deringing + EASU on the phone and in Edge on the
desktop; Firefox on the desktop, run with version 2 of the page (before the
deringing pass), lost none with bilinear, SGSR, FSR EASU and Anime4K S and M.
The desktop's first size is measured while its clocks still rise, so its
2560x1440 figures are above its 3840x2160 ones (deringing + SGSR 0.12 ms
there). For comparison, Firefox on Linux drawing in software (llvmpipe) took
1.8 ms for bilinear and 7.4 ms for deringing + SGSR: the page keeps the plain
path in such browsers, as planned.

What the runs taught about measuring:

- Version 1 of the page drew every frame of a measure into one target. A
  software rasterizer (llvmpipe, in Firefox on Linux) dropped full-screen draws
  overdrawn by the next one and reported 0.2 ms for bilinear; with frames into
  two targets in turn it reports 1.8 ms. Version 2 also left the programs'
  compilation and a second of warm-up out of the 30 s count (Anime4K S had lost
  13 frames to it on the phone).
- Firefox names the desktop's GPU as "GeForce GTX 980 ..., or similar" (its
  fingerprinting protection); Edge names the RTX 3080 Ti. Firefox's figures at
  3840x2160 stay under that card's fill rate, so its throughput timing is not
  trusted; its 30 s runs are.

## Gate 0

- Desktop GPUs: deringing (sigma 16 levels) + SGSR 1 (edge sharpness 2.0).
  Real frame: SSIM 0.938 against bilinear's 0.906, rise 1.78 against 2.34 px
  (-24 %), halo added +1. Cost 0.27 ms per 2560x1440 frame on an RTX 3080 Ti,
  no frame lost.
- Phone GPUs: the same. Cost 1.09 ms per 2560x1440 frame and 1.03 ms at the
  phone's screen size on an Adreno 830, no frame lost.

Both classes pass by SSIM (the rise is 23 to 28 % narrower). Shipping it
needs only SGSR's code (BSD-3-Clause) and the deringing pass (ours); FSR and
Anime4K are not shipped.

Limits: one phone, a 2024 flagship; slower phone GPUs are left to the
watchdog of phase 6. No iPhone and no Safari. The real pair is one target with
one chain whose levels clip. The ringing measure is the 95th percentile on two
panels; the crops agree with it.

## Next

Phase 1 builds the renderer in `web/src/lib/upscale/` with these two passes;
sharpness low, medium and high (phase 6) map to the deringing sigma and SGSR's
edge sharpness.
