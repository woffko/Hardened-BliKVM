// The measuring side of the upscaler bench (tools/upscale/, docs/baselines/work-plan-20261008-screen-upscaling.md,
// phase 0): the downscales a capture bridge may make, and the measures of a picture against its original. They
// decide the gate, so each is checked on pictures whose answer is known.
import assert from "node:assert/strict";
import test from "node:test";
import { translateAnime4K } from "./upscale/candidates.js";
import { downscale, luma } from "./upscale/kernels.js";
import { backgroundHalo, edgeMeasures, halo, psnr, ssim } from "./upscale/metrics.js";

/** RGBA bytes of a w x h grey picture given by f(x, y). */
function grey(w, h, f) {
  const out = new Uint8ClampedArray(w * h * 4);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const v = f(x, y);
      out.set([v, v, v, 255], 4 * (y * w + x));
    }
  }
  return out;
}

test("every downscale keeps a flat picture's level and a ramp's mean", () => {
  for (const kind of ["area", "bilinear", "bicubic"]) {
    const flat = downscale(grey(40, 20, () => 123), 40, 20, 30, 15, kind);
    assert.ok(flat.every((v, i) => (i % 4 === 3 ? v === 255 : v === 123)), kind);
    const ramp = downscale(grey(40, 4, (x) => x * 6), 40, 4, 30, 3, kind);
    const mean = (rgba) => luma(rgba).reduce((s, v) => s + v, 0) / (rgba.length / 4);
    assert.ok(Math.abs(mean(ramp) - 117) < 1, `${kind}: ${mean(ramp)}`);
  }
});

test("an area downscale by 4/3 averages exactly the pixels each output covers", () => {
  // Columns 0..3 of the source are 0, 0, 0, 255: output 0 covers 0..1.333, output 1 covers 1.333..2.667,
  // output 2 covers 2.667..4 (a third of column 2 and all of column 3).
  const out = downscale(grey(4, 1, (x) => (x === 3 ? 255 : 0)), 4, 1, 3, 1, "area");
  assert.deepEqual([out[0], out[4], out[8]], [0, 0, Math.round((255 * 1) / (4 / 3))]);
});

test("the stretched filters keep a flat level, and a moved grid moves the picture", () => {
  for (const kind of ["tent", "cubic-aa"]) {
    const flat = downscale(grey(40, 20, () => 77), 40, 20, 30, 15, kind);
    assert.ok(flat.every((v, i) => (i % 4 === 3 ? v === 255 : v === 77)), kind);
  }
  // A step at column 20 of 40, reduced to 20 columns by area: at x2 it falls between outputs 9 and 10; with the
  // grid moved by two source pixels it falls between outputs 8 and 9.
  const step = grey(40, 2, (x) => (x < 20 ? 0 : 255));
  const plain = downscale(step, 40, 2, 20, 1, "area");
  const moved = downscale(step, 40, 2, 20, 1, "area", 2, 0);
  assert.deepEqual([plain[4 * 9], plain[4 * 10]], [0, 255]);
  assert.deepEqual([moved[4 * 8], moved[4 * 9]], [0, 255]);
});

test("PSNR and SSIM of a picture against itself, and against a changed copy", () => {
  const a = luma(grey(64, 64, (x, y) => (x * 7 + y * 3) % 256));
  const rect = { x: 0, y: 0, w: 64, h: 64 };
  assert.equal(psnr(a, a, 64, rect), 99);
  assert.ok(Math.abs(ssim(a, a, 64, rect) - 1) < 1e-12);
  const b = a.map((v) => v + 5);
  assert.ok(Math.abs(psnr(a, b, 64, rect) - 10 * Math.log10((255 * 255) / 25)) < 1e-6);
  assert.ok(ssim(a, b, 64, rect) < 1);
});

test("a perfect step rises in 0.8 pixel and does not overshoot; a ringing one does", () => {
  const W = 40, H = 20;
  const edges = [{ orientation: "v", pos: 20, from: 4, to: 16, low: 0, high: 255 }];
  const step = luma(grey(W, H, (x) => (x < 20 ? 0 : 255)));
  const sharp = edgeMeasures(step, W, edges, 1);
  assert.equal(sharp.edges, 1);
  assert.ok(Math.abs(sharp.riseMedian - 0.8) < 1e-9);
  assert.equal(sharp.overshootP90, 0);
  // A step of 50 to 200 with a 12-level dip before it and a 12-level peak after it.
  const ringing = luma(grey(W, H, (x) => (x === 19 ? 38 : x === 20 ? 212 : x < 20 ? 50 : 200)));
  const rung = edgeMeasures(ringing, W, edges, 1);
  assert.ok(Math.abs(rung.overshootP90 - 24) < 1e-9, String(rung.overshootP90));
});

test("halo counts the background moved away from the text, not a blur toward it", () => {
  const W = 30, H = 30, rect = { x: 0, y: 0, w: W, h: H };
  // Light text (a bar at x 14..15) on a dark background of 30.
  const original = luma(grey(W, H, (x) => (x === 14 || x === 15 ? 230 : 30)));
  const blurred = luma(grey(W, H, (x) => (x === 14 || x === 15 ? 200 : x === 13 || x === 16 ? 60 : 30)));
  const ringed = luma(grey(W, H, (x) => (x === 14 || x === 15 ? 230 : x === 12 || x === 17 ? 20 : 30)));
  assert.equal(backgroundHalo(blurred, original, W, rect).p99, 0);
  const seen = backgroundHalo(ringed, original, W, rect);
  assert.equal(seen.background, 30);
  assert.ok(Math.abs(seen.p99 - 10) < 1e-3, String(seen.p99));
  assert.equal(halo(original, original, W, rect).p99, 0);
});

test("an Anime4K shader is cut into its passes with their textures and sizes", () => {
  const text = [
    "//!DESC first",
    "//!HOOK MAIN",
    "//!BIND MAIN",
    "//!SAVE conv2d_tf",
    "//!WIDTH MAIN.w",
    "vec4 hook() { return MAIN_texOff(vec2(0.0)); }",
    "//!DESC last",
    "//!HOOK MAIN",
    "//!BIND MAIN",
    "//!BIND conv2d_tf",
    "//!SAVE MAIN",
    "//!WIDTH conv2d_tf.w 2 *",
    "vec4 hook() { return conv2d_tf_tex(conv2d_tf_pos) + MAIN_tex(MAIN_pos); }",
  ].join("\n");
  const passes = translateAnime4K(text);
  assert.deepEqual(
    passes.map(({ binds, save, scale }) => ({ binds, save, scale })),
    [
      { binds: ["MAIN"], save: "conv2d_tf", scale: 1 },
      { binds: ["MAIN", "conv2d_tf"], save: "MAIN", scale: 2 },
    ],
  );
  assert.match(passes[1].source, /uniform sampler2D conv2d_tf_raw;/);
  assert.match(passes[1].source, /void main\(\) \{ outColor = hook\(\); \}/);
});
