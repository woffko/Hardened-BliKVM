// The phase-0 bench of docs/baselines/work-plan-20261008-screen-upscaling.md in a page. Quality: the synthetic
// 2560x1440 picture made smaller as a capture bridge makes it, enlarged back by every candidate and measured against
// itself. Cost: milliseconds per frame of each candidate on the GPU the page runs on, and frames lost while drawing a
// 30 frames a second feed. A lab tool, never shipped.
import { Bench, CANDIDATES } from "./candidates.js";
import { downscale, luma } from "./kernels.js";
import { backgroundHalo, edgeMeasures, halo, psnr, ssim } from "./metrics.js";
import { drawSynthetic } from "./patterns.js";

const W = 2560;
const H = 1440;

function context2d(w, h) {
  const canvas = new OffscreenCanvas(w, h);
  return canvas.getContext("2d", { willReadFrequently: true });
}

function webgl() {
  const canvas = document.createElement("canvas");
  const gl = canvas.getContext("webgl2", { antialias: false, alpha: false, depth: false, premultipliedAlpha: false });
  if (!gl) throw new Error("WebGL2 unavailable");
  return { canvas, gl };
}

/** The GPU and the page's surroundings, as the browser names them. */
export function deviceInfo(gl) {
  const debug = gl.getExtension("WEBGL_debug_renderer_info");
  return {
    userAgent: navigator.userAgent,
    renderer: debug ? gl.getParameter(debug.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER),
    vendor: debug ? gl.getParameter(debug.UNMASKED_VENDOR_WEBGL) : gl.getParameter(gl.VENDOR),
    floatTargets: Boolean(gl.getExtension("EXT_color_buffer_float")),
    maxTexture: gl.getParameter(gl.MAX_TEXTURE_SIZE),
    screen: [screen.width, screen.height],
    devicePixelRatio: devicePixelRatio,
  };
}

/** RGBA bytes after a JPEG round trip through the browser's encoder and decoder. */
async function jpegRoundTrip(rgba, w, h, quality) {
  const ctx = context2d(w, h);
  ctx.putImageData(new ImageData(new Uint8ClampedArray(rgba), w, h), 0, 0);
  const blob = await ctx.canvas.convertToBlob({ type: "image/jpeg", quality });
  const bitmap = await createImageBitmap(blob, { colorSpaceConversion: "none", premultiplyAlpha: "none" });
  const out = context2d(w, h);
  out.drawImage(bitmap, 0, 0);
  bitmap.close();
  return { data: out.getImageData(0, 0, w, h).data, bytes: blob.size };
}

/** A sheet of crops (rows: the original and each candidate; columns: regions), magnified, as a PNG data URL. */
async function cropSheet(rows, regions, zoom) {
  const label = 18;
  const cellW = Math.max(...regions.map((r) => r.w)) * zoom;
  const cellH = Math.max(...regions.map((r) => r.h)) * zoom;
  const ctx = context2d(regions.length * (cellW + 8) + 8, rows.length * (cellH + label + 8) + 8);
  ctx.fillStyle = "#777";
  ctx.fillRect(0, 0, ctx.canvas.width, ctx.canvas.height);
  ctx.imageSmoothingEnabled = false;
  rows.forEach((row, r) => {
    const y = 8 + r * (cellH + label + 8);
    ctx.fillStyle = "#fff";
    ctx.font = "14px sans-serif";
    ctx.fillText(row.name, 8, y + 14);
    regions.forEach((region, c) => {
      const part = context2d(region.w, region.h);
      const image = part.createImageData(region.w, region.h);
      for (let j = 0; j < region.h; j++) {
        const from = 4 * ((region.y + j) * W + region.x);
        image.data.set(row.rgba.subarray(from, from + 4 * region.w), 4 * j * region.w);
      }
      part.putImageData(image, 0, 0);
      ctx.drawImage(part.canvas, 8 + c * (cellW + 8), y + label, region.w * zoom, region.h * zoom);
    });
  });
  const blob = await ctx.canvas.convertToBlob({ type: "image/png" });
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let text = "";
  for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return `data:image/png;base64,${btoa(text)}`;
}

// The regions of the synthetic picture the crop sheets show: small text on light and on dark, the interface, lines.
const REGIONS = [
  { x: 8, y: 30, w: 220, h: 90 },
  { x: 1288, y: 30, w: 220, h: 90 },
  { x: 20, y: 590, w: 220, h: 90 },
  { x: 1290, y: 490, w: 220, h: 90 },
];

/** RGBA bytes of an image file the page can fetch (PNG or JPEG), decoded by the browser. */
async function decodeUrl(url) {
  const bitmap = await createImageBitmap(await (await fetch(url)).blob(), { colorSpaceConversion: "none", premultiplyAlpha: "none" });
  const ctx = context2d(bitmap.width, bitmap.height);
  ctx.drawImage(bitmap, 0, 0);
  const result = { w: bitmap.width, h: bitmap.height, data: ctx.getImageData(0, 0, bitmap.width, bitmap.height).data };
  bitmap.close();
  return result;
}

/**
 * The original: the synthetic picture drawn here, or the PNG it was saved as (`reference`); with its geometry. For a
 * captured frame the original is taken as the capture chain delivered it: its levels mapped by `levels`, and moved
 * by `shift` (whole pixels right and down) with its edges.
 */
async function originalPicture(reference, levels = null, shift = [0, 0]) {
  const original = context2d(W, H);
  let { edges, panels } = drawSynthetic(original, W, H);
  let orig = reference ? (await decodeUrl(reference)).data : original.getImageData(0, 0, W, H).data;
  if (levels) orig = mapLevels(orig, levels);
  const [sx, sy] = shift;
  if (sx || sy) {
    const moved = new Uint8ClampedArray(orig.length);
    for (let y = 0; y < H; y++) {
      const from = Math.min(H - 1, Math.max(0, y - sy));
      for (let x = 0; x < W; x++) {
        const at = 4 * (from * W + Math.min(W - 1, Math.max(0, x - sx)));
        moved.set(orig.subarray(at, at + 4), 4 * (y * W + x));
      }
    }
    orig = moved;
    edges = edges.map((e) =>
      e.orientation === "v"
        ? { ...e, pos: e.pos + sx, from: e.from + sy, to: e.to + sy }
        : { ...e, pos: e.pos + sy, from: e.from + sx, to: e.to + sx },
    );
  }
  return { edges, panels, orig };
}

/**
 * Quality: for each variant, a source enlarged back to 2560x1440 by each candidate and measured against the
 * original. A variant is either { name, w, h, kind, jpeg } (the original made w x h by `kind`, and JPEG at quality
 * `jpeg`) or { name, url } (a captured frame). `reference` is the original's PNG when it was drawn elsewhere;
 * `levels` and `shift` take it as a capture chain delivered it (see originalPicture). Run captured frames on
 * their own: the synthetic variants are made from the same, then changed, original.
 */
export async function runQuality({
  anime4k = {},
  variants,
  candidates = Object.keys(CANDIDATES),
  crops = [],
  reference = null,
  levels = null,
  shift = [0, 0],
}) {
  const { edges, panels, orig } = await originalPicture(reference, levels, shift);
  const origLuma = luma(orig);
  const { gl } = webgl();
  const bench = new Bench(gl, anime4k);
  const results = { device: deviceInfo(gl), edges: edges.length, panels, rows: [], sheets: {} };
  results.reference = { edges: edgeMeasures(origLuma, W, edges, 1) };
  for (const variant of variants) {
    let source, jpegBytes = null;
    if (variant.url) {
      const frame = await decodeUrl(variant.url);
      ({ data: source, w: variant.w, h: variant.h } = frame);
    } else {
      source = downscale(orig, W, H, variant.w, variant.h, variant.kind);
      if (variant.jpeg) ({ data: source, bytes: jpegBytes } = await jpegRoundTrip(source, variant.w, variant.h, variant.jpeg));
    }
    bench.upload(source, variant.w, variant.h);
    const sheetRows = crops.includes(variant.name) ? [{ name: "original 2560x1440", rgba: orig }] : null;
    for (const name of candidates) {
      if (CANDIDATES[name][0].anime4k && !bench.floatTargets) continue;
      const started = performance.now();
      const target = bench.run(name, W, H);
      const out = bench.read(target);
      const elapsed = performance.now() - started;
      const outLuma = luma(out);
      const row = { variant: variant.name, candidate: name, jpegBytes, elapsedMs: Math.round(elapsed), panels: {} };
      for (const panel of panels) {
        row.panels[panel.name] = {
          psnr: psnr(origLuma, outLuma, W, panel),
          ssim: ssim(origLuma, outLuma, W, panel),
          halo: halo(outLuma, origLuma, W, panel),
          backgroundHalo: backgroundHalo(outLuma, origLuma, W, panel),
        };
      }
      row.edges = edgeMeasures(outLuma, W, edges, 1);
      results.rows.push(row);
      if (sheetRows) sheetRows.push({ name, rgba: out });
    }
    if (sheetRows) results.sheets[variant.name] = await cropSheet(sheetRows, REGIONS, 3);
  }
  return results;
}

/** RGBA bytes with each channel mapped by a(v - b), clipped: the levels a capture chain applied. */
export function mapLevels(rgba, { a, b }) {
  const out = new Uint8ClampedArray(rgba.length);
  for (let i = 0; i < rgba.length; i++) out[i] = i % 4 === 3 ? 255 : Math.round(a * (rgba[i] - b));
  return out;
}

/**
 * The real pair: which frames show the picture (PSNR of their luma against the original's area downscale), and for
 * the best of them the levels the capture chain applied (a straight line a(v - b) fitted on flat areas, away from
 * the clipped ends), then which downscale the bridge makes and where its sampling grid lies (PSNR against each
 * kind of downscale of the level-mapped original, the grid moved in quarter pixels), and how far its blur reaches
 * (the edges' rise in the frame).
 */
export async function runRealPair({ reference, frames, kinds = ["area", "tent", "bilinear", "bicubic", "cubic-aa"] }) {
  const { edges, panels, orig } = await originalPicture(reference);
  const w = 1920, h = 1080;
  const area = luma(downscale(orig, W, H, w, h, "area"));
  const whole = { x: 0, y: 0, w, h };
  const result = { frames: [], fits: {} };
  let best = null;
  for (const url of frames) {
    const frame = await decodeUrl(url);
    if (frame.w !== w || frame.h !== h) {
      result.frames.push({ url, size: `${frame.w}x${frame.h}` });
      continue;
    }
    const y = luma(frame.data);
    const score = psnr(area, y, w, whole);
    result.frames.push({ url, psnr: score });
    if (!best || score > best.score) best = { url, score, y };
  }
  if (!best || best.score < 15) return result;
  result.chosen = best.url;
  // Levels, on flat areas of the downscale (5x5 range under 2) whose level lies in 24..216, compared a pixel
  // either way (the frame may sit a pixel off).
  {
    const pairs = [];
    for (let y = 4; y < h - 4; y += 2) {
      for (let x = 4; x < w - 4; x += 2) {
        let lo = 255, hi = 0;
        for (let j = -3; j <= 3; j++) {
          for (let i = -3; i <= 3; i++) {
            const v = area[(y + j) * w + x + i];
            if (v < lo) lo = v;
            if (v > hi) hi = v;
          }
        }
        const v = area[y * w + x];
        if (hi - lo < 2 && v >= 24 && v <= 216) pairs.push([v, best.y[y * w + x]]);
      }
    }
    let sx = 0, sy = 0, sxx = 0, sxy = 0;
    for (const [p, q] of pairs) {
      sx += p; sy += q; sxx += p * p; sxy += p * q;
    }
    const n = pairs.length;
    const a = (n * sxy - sx * sy) / (n * sxx - sx * sx);
    const c = (sy - a * sx) / n;
    let residual = 0;
    for (const [p, q] of pairs) residual += (q - (a * p + c)) ** 2;
    result.levels = { a, b: -c / a, pairs: n, rms: Math.sqrt(residual / n) };
  }
  const mapped = mapLevels(orig, result.levels);
  const scale = (rect) => ({ x: Math.round(rect.x * 0.75), y: Math.round(rect.y * 0.75), w: Math.round(rect.w * 0.75), h: Math.round(rect.h * 0.75) });
  const regions = panels.filter((p) => ["edges", "lines", "text-dark-on-light", "text-light-on-dark"].includes(p.name)).map((p) => ({ name: p.name, ...scale(p) }));
  const score = (down, region) => {
    let sum = 0, n = 0;
    for (let y = region.y + 2; y < region.y + region.h - 2; y++) {
      for (let x = region.x + 2; x < region.x + region.w - 2; x++) {
        const d = best.y[y * w + x] - down[y * w + x];
        sum += d * d;
        n++;
      }
    }
    return 10 * Math.log10((255 * 255) / (sum / n));
  };
  for (const kind of kinds) {
    // The grid's position on a coarse then a fine step, in source pixels, scored on all regions together.
    let top = { psnr: -1, dx: 0, dy: 0 };
    const tryAt = (dx, dy) => {
      const down = luma(downscale(mapped, W, H, w, h, kind, dx, dy));
      const all = regions.reduce((s, region) => s + score(down, region), 0) / regions.length;
      if (all > top.psnr) top = { psnr: all, dx, dy, regions: Object.fromEntries(regions.map((r) => [r.name, score(down, r)])) };
    };
    for (const dy of [-2, -1, 0, 1, 2]) for (const dx of [-2, -1, 0, 1, 2]) tryAt(dx, dy);
    const [cx, cy] = [top.dx, top.dy];
    for (const dy of [-0.75, -0.5, -0.25, 0, 0.25, 0.5, 0.75]) {
      for (const dx of [-0.75, -0.5, -0.25, 0, 0.25, 0.5, 0.75]) if (dx || dy) tryAt(cx + dx, cy + dy);
    }
    result.fits[kind] = top;
  }
  result.frameEdges = edgeMeasures(best.y, w, edges, 0.75);
  result.areaEdges = edgeMeasures(luma(downscale(mapped, W, H, w, h, "area")), w, edges, 0.75);
  return result;
}

/** The synthetic picture as a 1920x1080 frame (scaled by the browser), for the cost runs. */
async function costSource(w, h) {
  const original = context2d(W, H);
  drawSynthetic(original, W, H);
  const frame = context2d(w, h);
  frame.drawImage(original.canvas, 0, 0, w, h);
  return frame;
}

function sync(bench, target) {
  const gl = bench.gl;
  gl.bindFramebuffer(gl.FRAMEBUFFER, target ? target.fb : null);
  gl.readPixels(0, 0, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(4));
}

const pause = () => new Promise((resolve) => setTimeout(resolve, 0));

/** GPU time of `draw` (median of `count` frames, in ms) by EXT_disjoint_timer_query_webgl2, or null without it. */
async function gpuTime(gl, timer, draw, count = 9) {
  if (!timer) return null;
  const queries = [];
  for (let i = 0; i < count; i++) {
    const query = gl.createQuery();
    gl.beginQuery(timer.TIME_ELAPSED_EXT, query);
    draw(i);
    gl.endQuery(timer.TIME_ELAPSED_EXT);
    queries.push(query);
  }
  const deadline = performance.now() + 5000;
  const times = [];
  for (const query of queries) {
    while (!gl.getQueryParameter(query, gl.QUERY_RESULT_AVAILABLE)) {
      if (performance.now() > deadline) return null;
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
    times.push(gl.getQueryParameter(query, gl.QUERY_RESULT) / 1e6);
    gl.deleteQuery(query);
  }
  if (gl.getParameter(timer.GPU_DISJOINT_EXT)) return null;
  times.sort((a, b) => a - b);
  return times[Math.floor(times.length / 2)];
}

/**
 * Cost: for each output size and candidate, the mean time per frame on this GPU, the source already on it. Frames
 * go to two output targets in turn (a tiled GPU, or a software rasterizer, drops a full-screen draw overdrawn by the
 * next one in the same target), after two seconds of drawing that bring the GPU's clocks up. Draws that take under
 * 30 ms are timed in a row with one wait at the end (the GPU's throughput); slower ones one by one, so that no single
 * submission runs long enough for the browser's GPU watchdog. Each measure lasts about 1.5 s. Where the browser
 * offers GPU timer queries, each row also has the GPU's own time (`gpuMs`). Also the time to upload a 1920x1080
 * ImageBitmap, as a decoded frame would be.
 */
export async function runCost({ anime4k = {}, candidates = Object.keys(CANDIDATES), sizes, onProgress }) {
  const source = await costSource(1920, 1080);
  const { canvas, gl } = webgl();
  let lost = false;
  canvas.addEventListener("webglcontextlost", () => (lost = true));
  const bench = new Bench(gl, anime4k);
  const timer = gl.getExtension("EXT_disjoint_timer_query_webgl2");
  const results = { device: deviceInfo(gl), timerQuery: Boolean(timer), rows: [] };
  const pixels = source.getImageData(0, 0, 1920, 1080).data;
  bench.upload(pixels, 1920, 1080);
  const bitmaps = [await createImageBitmap(source.canvas), await createImageBitmap(source.canvas, { imageOrientation: "flipY" })];
  {
    gl.bindTexture(gl.TEXTURE_2D, bench.src);
    for (let i = 0; i < 5; i++) gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, gl.RGBA, gl.UNSIGNED_BYTE, bitmaps[i % 2]);
    sync(bench, null);
    const frames = 30;
    const started = performance.now();
    for (let i = 0; i < frames; i++) gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, gl.RGBA, gl.UNSIGNED_BYTE, bitmaps[i % 2]);
    sync(bench, null);
    results.uploadImageBitmapMs = (performance.now() - started) / frames;
    bench.upload(pixels, 1920, 1080);
  }
  {
    const until = performance.now() + 2000;
    for (let i = 0; performance.now() < until; i++) {
      const target = bench.run("bilinear", 3840, 2160, false, i % 2);
      if (i % 4 === 3) {
        sync(bench, target);
        await pause();
      }
    }
  }
  for (const [w, h] of sizes) {
    for (const name of candidates) {
      const row = { size: `${w}x${h}`, candidate: name, ms: null };
      results.rows.push(row);
      if (lost) {
        row.note = "context lost";
        continue;
      }
      if (CANDIDATES[name][0].anime4k && !bench.floatTargets) {
        row.note = "no float render targets";
        continue;
      }
      try {
        let target = null;
        let one = Infinity;
        for (let i = 0; i < 3; i++) {
          const started = performance.now();
          target = bench.run(name, w, h, false, i % 2);
          sync(bench, target);
          one = Math.min(one, performance.now() - started);
          await pause();
        }
        const frames = Math.max(5, Math.min(120, Math.round(1500 / Math.max(one, 0.5))));
        let total = 0;
        if (one < 30) {
          const started = performance.now();
          for (let i = 0; i < frames; i++) target = bench.run(name, w, h, false, i % 2);
          sync(bench, target);
          total = performance.now() - started;
        } else {
          for (let i = 0; i < frames; i++) {
            const started = performance.now();
            target = bench.run(name, w, h, false, i % 2);
            sync(bench, target);
            total += performance.now() - started;
            await pause();
          }
        }
        row.ms = total / frames;
        row.frames = frames;
        if (one < 30) row.gpuMs = await gpuTime(gl, timer, (i) => bench.run(name, w, h, false, i % 2));
        if (lost || gl.isContextLost()) row.note = "context lost";
      } catch (error) {
        row.note = String(error.message ?? error).slice(0, 300);
      }
      onProgress?.(results);
      await pause();
    }
  }
  bitmaps.forEach((b) => b.close());
  return results;
}

/**
 * Frames lost: a 30 frames a second feed (a timer alternating two frames) drawn by `candidate` on every animation
 * frame that has a new one, into a visible canvas of w x h device pixels, for `seconds`.
 */
export async function runSustained({ anime4k = {}, candidate, w, h, seconds = 30, host = document.body }) {
  const source = await costSource(1920, 1080);
  const bitmaps = [await createImageBitmap(source.canvas), await createImageBitmap(source.canvas, { imageOrientation: "flipY" })];
  const { canvas, gl } = webgl();
  canvas.width = w;
  canvas.height = h;
  canvas.style.width = `${w / devicePixelRatio}px`;
  canvas.style.height = `${h / devicePixelRatio}px`;
  canvas.style.transform = "scaleY(-1)";
  host.append(canvas);
  const bench = new Bench(gl, anime4k);
  bench.upload(source.getImageData(0, 0, 1920, 1080).data, 1920, 1080);
  // The programs compile and the GPU's clocks rise before the feed starts: a second of drawing, not counted.
  {
    const until = performance.now() + 1000;
    while (performance.now() < until) {
      bench.run(candidate, w, h, true);
      gl.readPixels(0, 0, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(4));
      await new Promise((resolve) => requestAnimationFrame(resolve));
    }
  }
  let arrived = 0, drawn = 0, pending = false, longest = 0, last = performance.now();
  const timer = setInterval(() => {
    arrived++;
    pending = true;
  }, 1000 / 30);
  await new Promise((resolve) => {
    const started = performance.now();
    const frame = (now) => {
      longest = Math.max(longest, now - last);
      last = now;
      if (pending) {
        pending = false;
        gl.bindTexture(gl.TEXTURE_2D, bench.src);
        gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, gl.RGBA, gl.UNSIGNED_BYTE, bitmaps[arrived % 2]);
        bench.run(candidate, w, h, true);
        drawn++;
      }
      if (now - started < seconds * 1000) requestAnimationFrame(frame);
      else resolve();
    };
    requestAnimationFrame(frame);
  });
  clearInterval(timer);
  canvas.remove();
  bitmaps.forEach((b) => b.close());
  return { candidate, size: `${w}x${h}`, seconds, arrived, drawn, lost: arrived - drawn, longestFrameMs: Math.round(longest) };
}
