# The video modes drawn darker than MJPEG: the colour range (2026-10-06)

The operator saw the hardware H.264 encoder's picture as darker than it should
be. This note records what was measured, what was changed and what is left.
Receipts are under `evidence/private/colour-range-20261006` (not in Git).

## What was found

The hardware encoder does not change the picture. A key frame the board's
video engine made and the very capture JPEG it was made from (matched by
capture time on the board's monotonic clock, 2 ms apart; luma PSNR 49.3 dB,
against 20 to 33 dB for the frames around it) were decoded by Chromium, and
the decoded planes compared with the I420 the encoder was given (the product's
own JPEG conversion of that JPEG). The same JPEG went through the product's
x264 and VP8 on the host:

| | Mean difference to the encoder's input, Y / U / V (levels) | Luma PSNR | Drawn, minus the JPEG drawn (mean luma) |
| --- | --- | --- | --- |
| H.264, video engine | -0.02 / +0.03 / +0.04 | 49.3 dB | -5.8 |
| H.264, x264 | +0.04 / +0.05 / +0.04 | 46.5 dB | -5.7 |
| VP8 | -0.02 / +0.10 / +0.02 | 44.6 dB | -5.8 |

Every video mode was drawn darker than MJPEG, by the same amount: the
capture's JPEG YCbCr is of the full range (0 to 255) with the BT.601 matrix,
the encoders pass it on as it is, and no stream said so. A decoder then takes
the narrow range (16 to 235) and the BT.709 matrix (Chromium's WebCodecs said
so of every one of the three streams) and stretches the picture: 16 and below
become black, 27 becomes 13, 64 becomes 56, 128 stays near 130, 200 becomes 214,
235 and above white, and the colours shift with the other matrix. On the
board's dark desktop the median luma was 12 against 27 as MJPEG.

## What was changed

- `b5c2a64`: the video engine's SPS has a video signal type: full range, and a
  colour description of BT.709 primaries (which sRGB shares), the sRGB
  transfer curve and the BT.601 matrix (H.264 tables E-3 to E-5), the colour
  space Chromium calls JPEG's. Test: the SPS read back (before: no signal
  type).
- `73ec155`: the x264 shim sets the same VUI; x264 only writes it. Test: the
  colour description read from x264's SPS for both presets, held to the
  engine's constants (before: none).
- `3d0d118`: the page's QUIC decoder is told the same colour space, so that a
  board whose encoder is older (every system up to beta.1's) draws QUIC right
  after an application update. Tests: the decoder's configuration
  (`tools/test_quic_h264.mjs`), and the QUIC contract reads the frame source's
  dark squares (Y 16) from the drawn page: 0.0 with neither fix, 16 with the
  old x264 and the new page, with the new x264 and the old page, and with
  both.
- Chromium was also checked on the host for which colour description makes
  the drawn frame the JPEG drawn: told full range and the BT.601 matrix, the
  key frame of the table above is within 0.2 levels of the JPEG on average;
  every variant of primaries and transfer was taken as JPEG's colour space.

## On the board

The test board (slot 74, then 75 `0.1.0-beta.2` from `3d0d118`, and the encoder
`a09d9750` of the same source), the page in each video mode in Chromium with
screenshots of the picture matched with the MJPEG frame each looks most like,
twelve screenshots a run, all matched (the target's screen was still):

| Path | Before (slot 74, encoder `8ca21dca`) | New page, old encoder | New page, new encoder |
| --- | --- | --- | --- |
| H.264 over QUIC, engine | -4.8 / -5.2 / -10.0, luma 13 / 13 / 61 | +0.04 / -0.13 / +0.06, luma 17 / 17 / 76 | 0.00 / -0.12 / +0.04, luma 17 / 17 / 76 |
| H.264 over WebRTC, engine | -4.7 / -5.1 / -9.9, luma 13 / 13 / 61 | -5.2 / -3.0 / -11.0, luma 13 / 13 / 61 | +0.13 / -0.03 / +0.14, luma 17 / 17 / 76 |
| H.264 over QUIC, x264 | | | +0.13 / -0.24 / +0.17, luma 16 / 17 / 78 |
| H.264 over WebRTC, x264 | | | +0.24 / -0.05 / +0.25, luma 17 / 17 / 78 |
| VP8 over WebRTC | -8.4 / -0.5 / -9.9, luma 12 / 12 / 76 | | -10.0 / -0.6 / -11.8, luma 11 / 12 / 71 |

The figures are the screen's mean red, green and blue minus the JPEG's, and
the screen's luma at its 10th, 50th and 90th percentile, against 17 / 17 / 76
of the JPEG (16 / 17 / 77 and 17 / 17 / 72 for the runs whose picture changed
a little). Chromium's WebRTC decoder takes the colour description from the
stream: the WebRTC rows are right only with the new encoder. A key frame of
the fixed engine, decoded without being told anything, comes out as full
range with the BT.601 matrix and within 0.4 levels of its JPEG.

x264 was measured with the board's choice set to H.264 SW for the two runs and
set back to the engine after them. The release's regression passed (GUI walks
at a desktop's and a phone's size, the desktop one with the monitor open, H.264
and VP8 over WebRTC, the QUIC picture, the phone's reach, the engine switched
from the menu once each way over QUIC and WebRTC within 0.41 s). The board's
settings are as before field by field, the engine choice is hardware, no file
or process of the work is on it, and its kernel log has no line from it.

## What is left

- VP8 has no field for the range: it is still drawn darker. Converting the
  capture's samples to the narrow range for VP8 alone would cost a pass over
  every frame and a seventh of the levels; not done.
- The encoder is part of the system, not of the application: a board gets the
  fixed encoder, and WebRTC H.264 drawn right, with a new card or a RAW system
  update. An application update brings the QUIC half.
