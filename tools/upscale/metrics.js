// Measures of an upscaled picture against the original (docs/baselines/work-plan-20261008-screen-upscaling.md,
// phase 0): PSNR and SSIM of the luma per panel, and at the step edges the 10-90 % rise width and the overshoot
// (halo). A lab tool, never shipped.

/** PSNR in dB of two luma planes (Float32Array, same size) inside a rectangle. */
export function psnr(a, b, W, rect) {
  let sum = 0, n = 0;
  for (let y = rect.y; y < rect.y + rect.h; y++) {
    for (let x = rect.x; x < rect.x + rect.w; x++) {
      const d = a[y * W + x] - b[y * W + x];
      sum += d * d;
      n++;
    }
  }
  const mse = sum / n;
  return mse === 0 ? 99 : 10 * Math.log10((255 * 255) / mse);
}

/** Mean SSIM of two luma planes inside a rectangle, 8x8 windows every 4 pixels. */
export function ssim(a, b, W, rect) {
  const c1 = (0.01 * 255) ** 2, c2 = (0.03 * 255) ** 2;
  let total = 0, count = 0;
  for (let y = rect.y; y + 8 <= rect.y + rect.h; y += 4) {
    for (let x = rect.x; x + 8 <= rect.x + rect.w; x += 4) {
      let ma = 0, mb = 0;
      for (let j = 0; j < 8; j++) for (let i = 0; i < 8; i++) { const k = (y + j) * W + x + i; ma += a[k]; mb += b[k]; }
      ma /= 64; mb /= 64;
      let va = 0, vb = 0, cov = 0;
      for (let j = 0; j < 8; j++) for (let i = 0; i < 8; i++) {
        const k = (y + j) * W + x + i, da = a[k] - ma, db = b[k] - mb;
        va += da * da; vb += db * db; cov += da * db;
      }
      va /= 63; vb /= 63; cov /= 63;
      total += ((2 * ma * mb + c1) * (2 * cov + c2)) / ((ma * ma + mb * mb + c1) * (va + vb + c2));
      count++;
    }
  }
  return total / count;
}

/**
 * Halo: how far the picture goes beyond the original's own range around each pixel (its local minimum and maximum),
 * in levels, at the pixels where that range exceeds 40 levels (text and lines). The range is taken over 5x5 pixels,
 * so that the blur next to an edge stays inside it; ringing and over-sharpening leave it. Returns the mean, the
 * 95th and the 99th percentile.
 */
export function halo(out, orig, W, rect) {
  const values = [];
  for (let y = Math.max(2, rect.y); y < rect.y + rect.h - 2; y++) {
    for (let x = Math.max(2, rect.x); x < rect.x + rect.w - 2; x++) {
      let lo = 255, hi = 0;
      for (let j = -2; j <= 2; j++) {
        for (let i = -2; i <= 2; i++) {
          const v = orig[(y + j) * W + x + i];
          if (v < lo) lo = v;
          if (v > hi) hi = v;
        }
      }
      if (hi - lo <= 40) continue;
      const v = out[y * W + x];
      values.push(Math.max(0, v - hi, lo - v));
    }
  }
  values.sort((p, q) => p - q);
  const at = (q) => values[Math.min(values.length - 1, Math.floor(values.length * q))];
  return { pixels: values.length, mean: values.reduce((s, v) => s + v, 0) / values.length, p95: at(0.95), p99: at(0.99) };
}

/**
 * Halo as it is seen: the background next to text (within 2 pixels in the original) moved away from the text's
 * side, lighter around dark text and darker around light text, in levels. The background is the rectangle's
 * commonest level in the original. A blur moves it toward the text and is not counted; neither is a stroke made
 * darker than the original's anti-aliased one, which `halo` above counts. Returns the mean, the 95th and the 99th
 * percentile.
 */
export function backgroundHalo(out, orig, W, rect) {
  const counts = new Uint32Array(256);
  for (let y = rect.y; y < rect.y + rect.h; y++) {
    for (let x = rect.x; x < rect.x + rect.w; x++) counts[Math.round(orig[y * W + x])]++;
  }
  const background = counts.indexOf(Math.max(...counts));
  const values = [];
  for (let y = Math.max(2, rect.y + 2); y < rect.y + rect.h - 2; y++) {
    for (let x = Math.max(2, rect.x + 2); x < rect.x + rect.w - 2; x++) {
      if (Math.abs(orig[y * W + x] - background) > 1) continue;
      let darker = false, lighter = false;
      for (let j = -2; j <= 2; j++) {
        for (let i = -2; i <= 2; i++) {
          const v = orig[(y + j) * W + x + i];
          if (v < background - 40) darker = true;
          if (v > background + 40) lighter = true;
        }
      }
      if (!darker && !lighter) continue;
      const v = out[y * W + x];
      values.push(Math.max(0, darker ? v - background : 0, lighter ? background - v : 0));
    }
  }
  values.sort((p, q) => p - q);
  const at = (q) => values[Math.min(values.length - 1, Math.floor(values.length * q))] ?? 0;
  return {
    background,
    pixels: values.length,
    mean: values.length ? values.reduce((s, v) => s + v, 0) / values.length : 0,
    p95: at(0.95),
    p99: at(0.99),
  };
}

/**
 * Rise width and overshoot at the step edges, for a picture scaled by `scale` against the original's geometry. The
 * profile across each edge is averaged along the edge, normalised between its own plateaus, and read for the
 * distance between its 10 % and 90 % crossings (in the original's pixels) and its excursion beyond the plateaus
 * (in levels).
 */
export function edgeMeasures(lumaPlane, W, edges, scale = 1) {
  const rises = [], overshoots = [];
  for (const e of edges) {
    const half = Math.round(7 * scale);
    const center = e.pos * scale; // the edge lies between pixels center-1 and center (in output pixels)
    const from = Math.round(e.from * scale), to = Math.round(e.to * scale);
    const profile = [];
    for (let d = -half; d < half; d++) {
      let s = 0, n = 0;
      for (let t = from; t < to; t++) {
        const x = e.orientation === "v" ? Math.round(center) + d : t;
        const y = e.orientation === "v" ? t : Math.round(center) + d;
        s += lumaPlane[y * W + x];
        n++;
      }
      profile.push(s / n);
    }
    const edgeAt = half; // index of the first pixel after the edge
    const low = (profile[0] + profile[1] + profile[2]) / 3;
    const high = (profile[profile.length - 1] + profile[profile.length - 2] + profile[profile.length - 3]) / 3;
    const span = high - low;
    if (Math.abs(span) < 40) continue;
    const norm = profile.map((v) => (v - low) / span);
    const crossing = (level) => {
      for (let i = 1; i < norm.length; i++) {
        if ((norm[i - 1] - level) * (norm[i] - level) <= 0 && norm[i - 1] !== norm[i]) {
          return i - 1 + (level - norm[i - 1]) / (norm[i] - norm[i - 1]);
        }
      }
      return NaN;
    };
    const x10 = crossing(0.1), x90 = crossing(0.9);
    if (Number.isFinite(x10) && Number.isFinite(x90)) rises.push(Math.abs(x90 - x10) / scale);
    const window = norm.slice(Math.max(0, edgeAt - Math.round(4 * scale)), edgeAt + Math.round(4 * scale));
    const over = Math.max(0, Math.max(...window) - 1) + Math.max(0, -Math.min(...window));
    overshoots.push(over * Math.abs(span));
  }
  const mean = (xs) => xs.reduce((s, v) => s + v, 0) / xs.length;
  const sorted = (xs) => xs.slice().sort((p, q) => p - q);
  return {
    edges: rises.length,
    riseMean: mean(rises),
    riseMedian: sorted(rises)[Math.floor(rises.length / 2)],
    overshootMean: mean(overshoots),
    overshootP90: sorted(overshoots)[Math.floor(overshoots.length * 0.9)],
  };
}
