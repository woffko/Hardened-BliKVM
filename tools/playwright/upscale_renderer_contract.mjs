// The page's upscaler (web/src/lib/upscale/, docs/design/screen-upscaling-v1.md) against its CPU reference
// (upscale_reference.mjs), in Chromium drawing with SwiftShader. A frame with text and the JPEG's ringing is drawn
// in every mode, with and without VP8's table, enlarged by 4/3, by 2 and by an uneven ratio, a part of it, made
// smaller, and given as an ImageBitmap, an <img> and a VideoFrame: every channel within one level of the reference. Frames of an unchanged size make no texture,
// framebuffer, program or buffer and reallocate nothing; a lost context draws nothing and says so, and once
// restored draws as before.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import http from "node:http";
import nodeModule from "node:module";
import path from "node:path";
import { launchChromium } from "./quality_fixture.mjs";

const root = path.resolve(import.meta.dirname, "../..");
// This Node reads the page's TypeScript by stripping its types.
const { stripTypeScriptTypes } = nodeModule;
if (typeof stripTypeScriptTypes !== "function") {
  console.log("Upscaler: skipped, this Node cannot strip TypeScript types");
  process.exit(0);
}
const typescript = async (file) =>
  stripTypeScriptTypes(
    await readFile(path.join(root, file), "utf8"),
  ).replaceAll(/from '\.\/([a-z-]+)\.ts'/g, "from './$1.js'");
const files = {
  "/shaders.js": await typescript("web/src/lib/upscale/shaders.ts"),
  "/upscaler.js": await typescript("web/src/lib/upscale/upscaler.ts"),
  "/reference.js": await readFile(
    path.join(import.meta.dirname, "upscale_reference.mjs"),
    "utf8",
  ),
  "/patterns.js": await readFile(
    path.join(root, "tools/upscale/patterns.js"),
    "utf8",
  ),
  "/kernels.js": await readFile(
    path.join(root, "tools/upscale/kernels.js"),
    "utf8",
  ),
  "/": "<!doctype html><title>upscaler</title><body></body>",
};
const server = http.createServer((request, response) => {
  const body = files[new URL(request.url, "http://localhost").pathname];
  response.writeHead(body ? 200 : 404, {
    "content-type": request.url === "/" ? "text/html" : "text/javascript",
  });
  response.end(body ?? "");
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));

const browser = await launchChromium({
  args: ["--enable-unsafe-swiftshader", "--use-angle=swiftshader"],
});
try {
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  if (process.env.UPSCALE_TRACE) {
    page.on("console", (message) => console.log("page:", message.text()));
  }
  await page.goto(`http://127.0.0.1:${server.address().port}/`);
  const result = await page.evaluate(async () => {
    const { Upscaler, MODES } = await import("/upscaler.js");
    const reference = await import("/reference.js");
    const { drawSynthetic } = await import("/patterns.js");
    const { downscale } = await import("/kernels.js");

    // The frame: the synthetic picture made 1920x1080 by area and JPEG at 0.75, a 240x135 part of its text.
    const original = new OffscreenCanvas(2560, 1440).getContext("2d", {
      willReadFrequently: true,
    });
    drawSynthetic(original, 2560, 1440);
    const small = downscale(
      original.getImageData(0, 0, 2560, 1440).data,
      2560,
      1440,
      1920,
      1080,
      "area",
    );
    const staged = new OffscreenCanvas(1920, 1080).getContext("2d");
    staged.putImageData(new ImageData(small, 1920, 1080), 0, 0);
    const jpeg = await staged.canvas.convertToBlob({
      type: "image/jpeg",
      quality: 0.75,
    });
    const decoded = await createImageBitmap(jpeg, {
      colorSpaceConversion: "none",
    });
    const [W, H] = [240, 135];
    const part = new OffscreenCanvas(W, H).getContext("2d", {
      willReadFrequently: true,
    });
    part.drawImage(decoded, 960, 40, W, H, 0, 0, W, H);
    const rgba = part.getImageData(0, 0, W, H).data;
    const frame = await createImageBitmap(part.canvas);

    // What the context makes, counted from here on.
    const counts = {};
    for (const name of [
      "createTexture",
      "createFramebuffer",
      "createProgram",
      "createShader",
      "createBuffer",
      "createVertexArray",
      "texImage2D",
    ]) {
      const original = WebGL2RenderingContext.prototype[name];
      WebGL2RenderingContext.prototype[name] = function (...args) {
        counts[name] = (counts[name] ?? 0) + 1;
        return original.apply(this, args);
      };
    }

    const canvas = document.createElement("canvas");
    document.body.append(canvas);
    const events = [];
    const upscaler = Upscaler.on(canvas, {
      onLost: () => events.push("lost"),
      onRestored: () => events.push("restored"),
    });
    const read = () => {
      const gl = upscaler.gl;
      const [w, h] = [gl.drawingBufferWidth, gl.drawingBufferHeight];
      const bottomUp = new Uint8Array(w * h * 4);
      gl.readPixels(0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, bottomUp);
      const out = new Uint8Array(w * h * 4);
      for (let y = 0; y < h; y++) {
        out.set(
          bottomUp.subarray((h - 1 - y) * w * 4, (h - y) * w * 4),
          y * w * 4,
        );
      }
      return out;
    };
    // Pixels whose edge test the reference found within 1e-4 of a level of
    // the threshold may take the other branch in single precision.
    const compare = (got, want, near) => {
      let worst = 0;
      let over = 0;
      let skipped = 0;
      for (let i = 0; i < want.length; i++) {
        if (i % 4 === 3) continue;
        if (near[i >> 2]) {
          if (i % 4 === 0) skipped++;
          continue;
        }
        const d = Math.abs(got[i] - want[i]);
        worst = Math.max(worst, d);
        if (d > 1) over++;
      }
      return { worst, over, skipped };
    };
    const cases = [];
    const run = (name, width, height, options, source = frame) => {
      canvas.width = width;
      canvas.height = height;
      const drawn = upscaler.draw(source, W, H, options);
      const got = read();
      const sigma =
        options.mode === "plain"
          ? options.vp8Range
            ? 0
            : null
          : MODES[options.mode].sigma;
      const middle =
        sigma === null
          ? rgba
          : reference.dering(rgba, W, H, sigma, Boolean(options.vp8Range));
      const want = reference.output(
        middle,
        W,
        H,
        width,
        height,
        options.mode === "plain" ? null : MODES[options.mode].edge,
        options.crop,
      );
      cases.push({ name, drawn, ...compare(got, want.rgba, want.near) });
      console.log(name, JSON.stringify(cases.at(-1)));
    };
    for (const mode of ["plain", "low", "medium", "high"]) {
      for (const vp8Range of [false, true]) {
        run(`${mode}${vp8Range ? " vp8" : ""} 4/3`, 320, 180, {
          mode,
          vp8Range,
        });
      }
      run(`${mode} 2x`, 480, 270, { mode });
      run(`${mode} uneven`, 391, 217, { mode });
      run(`${mode} part`, 360, 204, {
        mode,
        crop: { x: 30, y: 20, width: 120, height: 68 },
      });
    }
    run("plain smaller", 160, 90, { mode: "plain" });
    // The same frame as an <img> and as a VideoFrame.
    const image = new Image();
    image.src = URL.createObjectURL(
      await part.canvas.convertToBlob({ type: "image/png" }),
    );
    await image.decode();
    run("medium 4/3 from an img", 320, 180, { mode: "medium" }, image);
    const videoFrame = new VideoFrame(frame, { timestamp: 0 });
    run(
      "medium 4/3 from a VideoFrame",
      320,
      180,
      { mode: "medium" },
      videoFrame,
    );
    videoFrame.close();

    // Frames of an unchanged size make nothing.
    canvas.width = 320;
    canvas.height = 180;
    upscaler.draw(frame, W, H, { mode: "medium" });
    const before = { ...counts };
    for (let i = 0; i < 20; i++) upscaler.draw(frame, W, H, { mode: "medium" });
    const made = Object.fromEntries(
      Object.entries(counts).map(([k, v]) => [k, v - (before[k] ?? 0)]),
    );

    // A lost context draws nothing; restored, it draws as before.
    const lose = upscaler.gl.getExtension("WEBGL_lose_context");
    const event = (type) =>
      new Promise((resolve) =>
        canvas.addEventListener(type, resolve, { once: true }),
      );
    let lostEvent = event("webglcontextlost");
    lose.loseContext();
    await lostEvent;
    const whileLost = {
      drawn: upscaler.draw(frame, W, H, { mode: "medium" }),
      ready: upscaler.ready,
    };
    const restoredEvent = event("webglcontextrestored");
    // Restoring is allowed once the lost event has been handled.
    await new Promise((resolve) => setTimeout(resolve, 0));
    lose.restoreContext();
    await restoredEvent;
    await new Promise((resolve) => setTimeout(resolve, 0));
    run("medium 4/3 after restore", 320, 180, { mode: "medium" });
    return { cases, made, whileLost, events, ready: upscaler.ready };
  });
  assert.deepEqual(errors, []);
  for (const c of result.cases) {
    assert.ok(c.drawn, `${c.name}: not drawn`);
    assert.ok(
      c.worst <= 1,
      `${c.name}: ${c.over} channels more than one level off, worst ${c.worst}`,
    );
    assert.ok(
      c.skipped <= 2,
      `${c.name}: ${c.skipped} pixels at the threshold`,
    );
  }
  for (const [name, count] of Object.entries(result.made)) {
    assert.equal(
      count,
      0,
      `${name} called ${count} times for unchanged frames`,
    );
  }
  assert.deepEqual(result.whileLost, { drawn: false, ready: false });
  assert.deepEqual(result.events, ["lost", "restored"]);
  assert.equal(result.ready, true);
  console.log(
    `Upscaler: ${result.cases.length} cases within one level of the reference (worst ${Math.max(...result.cases.map((c) => c.worst))}), nothing made for unchanged frames, a lost context drawing nothing and drawing again once restored`,
  );
} finally {
  await browser.close();
  server.close();
}
