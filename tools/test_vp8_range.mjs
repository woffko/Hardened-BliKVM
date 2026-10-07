import assert from "node:assert/strict";
import test from "node:test";

import {
  softwareRenderer,
  vp8RangeTable,
  vp8RangeTableValues,
} from "../web/src/lib/vp8-range.ts";

// What a browser draws for a channel of the board's full-range samples: VP8
// is taken as of the narrow range.
const stretched = (level) =>
  Math.min(255, Math.max(0, Math.round((255 / 219) * (level - 16))));

test("the table draws the board's levels as the JPEG has them", () => {
  const table = vp8RangeTable();
  assert.equal(table.length, 256);
  // Every level the stretch keeps comes back within one.
  for (let level = 17; level <= 234; level += 1) {
    const shown = Math.round(table[stretched(level)] * 255);
    assert.ok(Math.abs(shown - level) <= 1, `${level} is drawn as ${shown}`);
  }
  // Black and white stay so; what the stretch made black or white is drawn
  // so.
  for (const level of [0, 5, 16]) assert.equal(table[stretched(level)], 0);
  for (const level of [235, 250, 255]) assert.equal(table[stretched(level)], 1);
  // Without the table the dark levels are lost: 27 was drawn as 13.
  assert.equal(stretched(27), 13);
  assert.equal(Math.round(table[13] * 255), 27);
  // It never goes back, and never leaves 0 to 1.
  for (let drawn = 1; drawn < 256; drawn += 1) {
    assert.ok(table[drawn] > table[drawn - 1], `${drawn}`);
  }
  assert.ok(table.every((value) => value >= 0 && value <= 1));
});

test("the transfer function has a value for every level", () => {
  const values = vp8RangeTableValues().split(" ");
  assert.equal(values.length, 256);
  assert.equal(values[0], "0.00000");
  assert.equal(values[1], "0.06611");
  assert.equal(values[128], "0.49384");
  assert.equal(values[255], "1.00000");
  assert.ok(values.every((value) => /^[01]\.\d{5}$/.test(value)));
});

test("software rasterizers are told from GPUs", () => {
  for (const renderer of [
    "ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero) (0x0000C0DE)), SwiftShader driver)",
    "Google SwiftShader",
    "llvmpipe (LLVM 15.0.7, 256 bits)",
    "softpipe",
    "ANGLE (Microsoft, Microsoft Basic Render Driver Direct3D11 vs_5_0 ps_5_0, D3D11)",
  ]) {
    assert.equal(softwareRenderer(renderer), true, renderer);
  }
  for (const renderer of [
    "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 (0x00002503) Direct3D11 vs_5_0 ps_5_0, D3D11)",
    "ANGLE (Intel, Mesa Intel(R) UHD Graphics 620 (KBL GT2), OpenGL 4.6)",
    "ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)",
    "Apple GPU",
    "Mali-G78",
    "Adreno (TM) 650",
  ]) {
    assert.equal(softwareRenderer(renderer), false, renderer);
  }
});
