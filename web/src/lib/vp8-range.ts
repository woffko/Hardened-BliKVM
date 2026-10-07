/**
 * The page's correction of the board's VP8 picture. VP8 has no field for what
 * its samples mean, and browsers take them as of the narrow range (16 to 235)
 * with the BT.601 matrix. The board's samples are the capture's JPEG YCbCr,
 * of the full range (0 to 255), given to the encoder as they are: every
 * colour channel comes out stretched, drawn = 255 / 219 (level - 16), and the
 * picture darker than the MJPEG one. H.264 says what its samples mean; VP8
 * cannot, so the page undoes the stretch with a table per channel, an SVG
 * filter on the picture. The browser applies it as it draws each frame, and
 * the board does nothing more. With the machine's GPU that costs next to
 * nothing; drawn in software a filter on the picture costs a processor core
 * more at 1920x1080 and 30 frames a second, and frames (Chromium on
 * SwiftShader: 26 of 30 drawn, with any filter alike), so such a browser is
 * left drawing the picture as before.
 *
 * Black and white stay black and white: the stretch has made every level up
 * to 16 black and every level from 235 white, and those are the levels a
 * screen has most. The two matrices differ in the chroma's scale alone (224
 * levels in the narrow range, 255 in the full one), so per channel the
 * colours come out 2 % less saturated than the MJPEG ones; the table leaves
 * that.
 */

/** The id of the SVG filter that draws the board's VP8 picture right. */
export const VP8_RANGE_FILTER_ID = 'vp8-full-range';

/** For each level 0 to 255 a channel is drawn with, the level it should have, 0 to 1. */
export function vp8RangeTable(): number[] {
  const table: number[] = [];
  for (let drawn = 0; drawn < 256; drawn += 1) {
    if (drawn === 0 || drawn === 255) table.push(drawn / 255);
    else table.push(((drawn * 219) / 255 + 16) / 255);
  }
  return table;
}

/** The table as the `tableValues` of an SVG transfer function. */
export function vp8RangeTableValues(): string {
  return vp8RangeTable()
    .map((value) => value.toFixed(5))
    .join(' ');
}

/** Whether a WebGL renderer's name is of a software rasterizer. */
export function softwareRenderer(renderer: string): boolean {
  return /swiftshader|llvmpipe|softpipe|basic render driver|software/i.test(renderer);
}

let gpu: boolean | undefined;

/**
 * Whether this browser draws with the machine's GPU, judged once by the
 * renderer its WebGL names: no WebGL, or a software rasterizer, means no.
 */
export function drawsWithGpu(): boolean {
  if (gpu !== undefined) return gpu;
  gpu = false;
  try {
    const context = document.createElement('canvas').getContext('webgl');
    if (context) {
      const info = context.getExtension('WEBGL_debug_renderer_info');
      const renderer = context.getParameter(info ? info.UNMASKED_RENDERER_WEBGL : context.RENDERER);
      gpu = typeof renderer === 'string' && renderer !== '' && !softwareRenderer(renderer);
      context.getExtension('WEBGL_lose_context')?.loseContext();
    }
  } catch {
    gpu = false;
  }
  return gpu;
}
