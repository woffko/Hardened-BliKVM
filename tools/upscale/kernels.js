// Downscales of a 2560x1440 picture to 1920x1080 as a capture bridge may make them, and the luma helpers the
// measures use. A lab tool for docs/baselines/work-plan-20261008-screen-upscaling.md, never shipped.

function catmullRom(t) {
  const a = Math.abs(t);
  if (a < 1) return 1.5 * a * a * a - 2.5 * a * a + 1;
  if (a < 2) return -0.5 * a * a * a + 2.5 * a * a - 4 * a + 2;
  return 0;
}

/**
 * Weights of each output sample along an axis: [[source index, weight], ...]. `offset` moves the sampling grid by
 * that many source pixels (a bridge whose picture starts early or late).
 */
function axisWeights(srcSize, dstSize, kind, offset = 0) {
  const scale = srcSize / dstSize;
  const rows = [];
  for (let o = 0; o < dstSize; o++) {
    const center = (o + 0.5) * scale + offset; // in source pixel units, centers at k + 0.5
    let taps = [];
    if (kind === "area") {
      const from = o * scale + offset, to = (o + 1) * scale + offset;
      for (let k = Math.floor(from); k < Math.ceil(to); k++) {
        const w = Math.min(to, k + 1) - Math.max(from, k);
        if (w > 0) taps.push([k, w]);
      }
    } else if (kind === "bilinear") {
      const p = center - 0.5, k = Math.floor(p), t = p - k;
      taps = [[k, 1 - t], [k + 1, t]];
    } else if (kind === "bicubic") {
      const p = center - 0.5, k = Math.floor(p), t = p - k;
      for (let j = -1; j <= 2; j++) taps.push([k + j, catmullRom(j - t)]);
    } else if (kind === "tent" || kind === "cubic-aa") {
      // Filters stretched by the reduction: a triangle as wide as two output pixels, or Catmull-Rom over four.
      const reach = kind === "tent" ? scale : 2 * scale;
      for (let k = Math.floor(center - reach); k <= Math.ceil(center + reach); k++) {
        const d = (k + 0.5 - center) / scale;
        const weight = kind === "tent" ? Math.max(0, 1 - Math.abs(d)) : catmullRom(d);
        if (weight !== 0) taps.push([k, weight]);
      }
    } else throw new Error(`unknown kernel ${kind}`);
    const sum = taps.reduce((s, [, w]) => s + w, 0);
    rows.push(taps.map(([k, w]) => [Math.min(srcSize - 1, Math.max(0, k)), w / sum]));
  }
  return rows;
}

/**
 * RGBA bytes of a W x H picture scaled to w x h by a separable kernel ("area", "bilinear", "bicubic", or the
 * stretched "tent" and "cubic-aa"), the sampling grid moved by `dx`, `dy` source pixels.
 */
export function downscale(rgba, W, H, w, h, kind, dx = 0, dy = 0) {
  const xw = axisWeights(W, w, kind, dx), yw = axisWeights(H, h, kind, dy);
  const mid = new Float32Array(w * H * 4);
  for (let y = 0; y < H; y++) {
    for (let x = 0; x < w; x++) {
      let r = 0, g = 0, b = 0;
      for (const [k, wt] of xw[x]) {
        const i = 4 * (y * W + k);
        r += rgba[i] * wt; g += rgba[i + 1] * wt; b += rgba[i + 2] * wt;
      }
      const o = 4 * (y * w + x);
      mid[o] = r; mid[o + 1] = g; mid[o + 2] = b;
    }
  }
  const out = new Uint8ClampedArray(w * h * 4);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      let r = 0, g = 0, b = 0;
      for (const [k, wt] of yw[y]) {
        const i = 4 * (k * w + x);
        r += mid[i] * wt; g += mid[i + 1] * wt; b += mid[i + 2] * wt;
      }
      const o = 4 * (y * w + x);
      out[o] = Math.round(r); out[o + 1] = Math.round(g); out[o + 2] = Math.round(b); out[o + 3] = 255;
    }
  }
  return out;
}

/** Luma (BT.601 weights, as the earlier colour work used) of RGBA bytes. */
export function luma(rgba) {
  const out = new Float32Array(rgba.length / 4);
  for (let i = 0; i < out.length; i++) out[i] = 0.299 * rgba[4 * i] + 0.587 * rgba[4 * i + 1] + 0.114 * rgba[4 * i + 2];
  return out;
}
