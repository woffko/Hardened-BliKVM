# Encoder test frames

- `testsrc-64x48-420.jpg`, `testsrc-64x48-422.jpg`: small colourful test
  frames (FFmpeg's testsrc2) in 4:2:0 and 4:2:2.
- `pattern-320x180-{444,422,420,gray}.jpg`: one synthetic 320x180 picture
  (gradients, colour bars and one-pixel text-like strokes on a light panel),
  written as a PPM and compressed by cjpeg from libjpeg-turbo 3.2.0 at
  quality 85 with `-sample 1x1`, `2x1`, `2x2` and `-grayscale`. The picture is
  regenerated in `src/yuv.rs` (`pattern_ycbcr`) to compare the decoders with
  the source.
