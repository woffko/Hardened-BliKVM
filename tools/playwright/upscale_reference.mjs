// A CPU reference of the page's upscaler (web/src/lib/upscale/shaders.ts): the same deringing pass, SGSR 1 and
// bilinear pass, in double precision, for the renderer's contract to compare the GPU's output with. Frames are RGBA
// bytes, row 0 at the top. A test support module, never shipped.

const TO_LUMA = [0.299, 0.587, 0.114];

/** The VP8 table of web/src/lib/vp8-range.ts on a 0..1 level. */
function vp8(level) {
  if (level < 0.5 / 255 || level > 254.5 / 255) return level;
  return (level * 219 + 16) / 255;
}

/** The deringing pass: RGBA bytes of the same size (the GPU stores it in an 8-bit texture). */
export function dering(rgba, width, height, sigmaLevels, vp8Range) {
  const out = new Uint8ClampedArray(width * height * 4);
  const sigma = sigmaLevels / 255;
  const level = (x, y, c) => {
    const v =
      rgba[
        4 *
          (Math.min(height - 1, Math.max(0, y)) * width +
            Math.min(width - 1, Math.max(0, x))) +
          c
      ] / 255;
    return vp8Range ? vp8(v) : v;
  };
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const centre = [0, 1, 2].map((c) => level(x, y, c));
      if (sigma <= 0) {
        for (let c = 0; c < 3; c++)
          out[4 * (y * width + x) + c] = Math.round(centre[c] * 255);
        out[4 * (y * width + x) + 3] = 255;
        continue;
      }
      const centreLuma =
        centre[0] * TO_LUMA[0] +
        centre[1] * TO_LUMA[1] +
        centre[2] * TO_LUMA[2];
      const rangeScale = 1 / (2 * sigma * sigma);
      const sum = [0, 0, 0];
      let weights = 0;
      for (let j = -2; j <= 2; j++) {
        for (let i = -2; i <= 2; i++) {
          const c = [0, 1, 2].map((k) => level(x + i, y + j, k));
          const d =
            c[0] * TO_LUMA[0] +
            c[1] * TO_LUMA[1] +
            c[2] * TO_LUMA[2] -
            centreLuma;
          const w = Math.exp(-(i * i + j * j) / 4.5 - d * d * rangeScale);
          for (let k = 0; k < 3; k++) sum[k] += c[k] * w;
          weights += w;
        }
      }
      for (let k = 0; k < 3; k++)
        out[4 * (y * width + x) + k] = Math.round((sum[k] / weights) * 255);
      out[4 * (y * width + x) + 3] = 255;
    }
  }
  return out;
}

/** A texel's channel, 0..1, clamped to the edge. */
function texel(frame, width, height, x, y, c) {
  return (
    frame[
      4 *
        (Math.min(height - 1, Math.max(0, y)) * width +
          Math.min(width - 1, Math.max(0, x))) +
        c
    ] / 255
  );
}

/** The GPU's bilinear sample at a position in pixels (centres at k + 0.5), RGB 0..1. */
function bilinear(frame, width, height, px, py) {
  const fx = px - 0.5,
    fy = py - 0.5;
  const x0 = Math.floor(fx),
    y0 = Math.floor(fy);
  const tx = fx - x0,
    ty = fy - y0;
  return [0, 1, 2].map((c) => {
    const top =
      texel(frame, width, height, x0, y0, c) * (1 - tx) +
      texel(frame, width, height, x0 + 1, y0, c) * tx;
    const bottom =
      texel(frame, width, height, x0, y0 + 1, c) * (1 - tx) +
      texel(frame, width, height, x0 + 1, y0 + 1, c) * tx;
    return top * (1 - ty) + bottom * ty;
  });
}

function fastLanczos2(x) {
  let wA = x - 4;
  const wB = x * wA - wA;
  wA *= wA;
  return wB * wA;
}

function weightY(dx, dy, c, stdv) {
  const x =
    (dx * dx + dy * dy) * 0.55 + Math.min(1, Math.max(0, Math.abs(c) * stdv));
  const w = fastLanczos2(x);
  return [w, w * c];
}

/** SGSR's edge threshold in the page's shader (8.37 levels; see shaders.ts). */
const EDGE_THRESHOLD = 8.37 / 255;

/**
 * The last pass to an output of outWidth x outHeight: SGSR 1 with edge sharpness `edge`, or bilinear when `edge` is
 * null; `crop` the part of the frame drawn. Returns the RGBA bytes, row 0 at the top, and a mask of the pixels whose
 * edge test came within 1e-4 of a level of the threshold, where single and double precision may take different
 * branches.
 */
export function output(
  frame,
  width,
  height,
  outWidth,
  outHeight,
  edge,
  crop = { x: 0, y: 0, width, height },
) {
  const out = new Uint8ClampedArray(outWidth * outHeight * 4);
  const near = new Uint8Array(outWidth * outHeight);
  const green = (x, y) => texel(frame, width, height, x, y, 1);
  // textureGather at the texel corner (cx, cy): x = (cx - 1, cy), y = (cx, cy), z = (cx, cy - 1), w = (cx - 1, cy - 1).
  const gather = (cx, cy) => [
    green(cx - 1, cy),
    green(cx, cy),
    green(cx, cy - 1),
    green(cx - 1, cy - 1),
  ];
  for (let oy = 0; oy < outHeight; oy++) {
    for (let ox = 0; ox < outWidth; ox++) {
      const px = crop.x + ((ox + 0.5) / outWidth) * crop.width;
      const py = crop.y + ((oy + 0.5) / outHeight) * crop.height;
      const color = bilinear(frame, width, height, px, py);
      if (edge !== null) {
        const ix = px - 0.5,
          iy = py + 0.5;
        const cx0 = Math.floor(ix),
          cy = Math.floor(iy);
        const plx = ix - cx0,
          ply = iy - cy;
        let left = gather(cx0, cy);
        const edgeVote =
          Math.abs(left[2] - left[1]) +
          Math.abs(color[1] - left[1]) +
          Math.abs(color[1] - left[2]);
        if (Math.abs(edgeVote - EDGE_THRESHOLD) < 1e-4 / 255)
          near[oy * outWidth + ox] = 1;
        if (edgeVote > EDGE_THRESHOLD) {
          const cx = cx0 + 1;
          let right = gather(cx + 1, cy);
          const above = gather(cx, cy - 1),
            below = gather(cx, cy + 1);
          let upDown = [above[3], above[2], below[1], below[0]];
          const mean = (left[1] + left[2] + right[0] + right[3]) * 0.25;
          left = left.map((v) => v - mean);
          right = right.map((v) => v - mean);
          upDown = upDown.map((v) => v - mean);
          const centre = color[1] - mean;
          const sum = [...left, ...right, ...upDown].reduce(
            (s, v) => s + Math.abs(v),
            0,
          );
          const stdv = 2.181818 / sum;
          const terms = [
            [plx, ply + 1, upDown[0]],
            [plx - 1, ply + 1, upDown[1]],
            [plx - 1, ply - 2, upDown[2]],
            [plx, ply - 2, upDown[3]],
            [plx + 1, ply - 1, left[0]],
            [plx, ply - 1, left[1]],
            [plx, ply, left[2]],
            [plx + 1, ply, left[3]],
            [plx - 1, ply - 1, right[0]],
            [plx - 2, ply - 1, right[1]],
            [plx - 2, ply, right[2]],
            [plx - 1, ply, right[3]],
          ];
          let w = 0,
            wy = 0;
          for (const [dx, dy, c] of terms) {
            const [a, b] = weightY(dx, dy, c, stdv);
            w += a;
            wy += b;
          }
          const maxY = Math.max(left[1], left[2], right[0], right[3]);
          const minY = Math.min(left[1], left[2], right[0], right[3]);
          const finalY = Math.min(maxY, Math.max(minY, edge * (wy / w)));
          const deltaY = Math.min(
            23 / 255,
            Math.max(-23 / 255, finalY - centre),
          );
          for (let c = 0; c < 3; c++)
            color[c] = Math.min(1, Math.max(0, color[c] + deltaY));
        }
      }
      for (let c = 0; c < 3; c++)
        out[4 * (oy * outWidth + ox) + c] = Math.round(color[c] * 255);
      out[4 * (oy * outWidth + ox) + 3] = 255;
    }
  }
  return { rgba: out, near };
}
