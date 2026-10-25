// The board's VP8 picture drawn as its JPEG is (web/src/lib/vp8-range.ts):
// the board's samples are of the full range, VP8 cannot say so, and Chromium
// draws a VP8 frame as of the narrow range. The built page is given, in place
// of the WebRTC connection, a track of such frames (the board's samples,
// marked as Chromium marks a decoded VP8 frame) and must draw them as the
// JPEG with those samples is drawn; in H.264 over the same view, whose stream
// says what its samples mean, it must draw them unchanged. A browser that
// draws in software is left drawing VP8 as before (a filter costs it a core):
// headless Chromium here is one (SwiftShader), and stands in for a browser
// with a GPU by the renderer name its WebGL is made to give. A browser that
// names a GPU but has no WebGL2 draws VP8 through the table as before,
// without upscaling or a notice.
import assert from "node:assert/strict";
import { defaultManifest, startApp } from "./app_harness.mjs";
import { launchChromium } from "./quality_fixture.mjs";

const browser = await launchChromium();
const app = await startApp();
const MODE_KEY = "hardened-blikvm-video-mode";
const manifest = defaultManifest();
manifest.video = {
  ...manifest.video,
  transports: ["mjpeg", "web_rtc_janus", "web_rtc_janus_h264"],
};

// The board's samples (Y, Cb, Cr) in patches: greys from black to white, and
// colours of a desktop.
const PATCHES = [
  [0, 128, 128],
  [16, 128, 128],
  [17, 128, 128],
  [27, 128, 128],
  [48, 128, 128],
  [64, 128, 128],
  [128, 128, 128],
  [166, 128, 128],
  [200, 128, 128],
  [234, 128, 128],
  [235, 128, 128],
  [255, 128, 128],
  [60, 160, 110],
  [120, 90, 170],
  [40, 140, 120],
  [200, 120, 135],
];
const COLUMNS = 4;
const WIDTH = 1280;
const HEIGHT = 720;

/** The JPEG's drawing of a patch: JFIF YCbCr, full range, BT.601. */
const jpeg = ([y, cb, cr]) =>
  [
    y + 1.402 * (cr - 128),
    y - 0.344136 * (cb - 128) - 0.714136 * (cr - 128),
    y + 1.772 * (cb - 128),
  ].map((value) => Math.round(Math.min(255, Math.max(0, value))));

// A GPU's renderer, as Chromium on Windows names it.
const GPU =
  "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 (0x00002503) Direct3D11 vs_5_0 ps_5_0, D3D11)";

async function open(
  mode,
  marked,
  renderer,
  { upscale = "off", ratio = 1, sent = null, from = browser } = {},
) {
  const context = await from.newContext({
    viewport: { width: WIDTH, height: HEIGHT },
    deviceScaleFactor: ratio,
  });
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  await context.addInitScript(
    ([
      key,
      mode,
      patches,
      columns,
      width,
      height,
      marked,
      renderer,
      upscale,
    ]) => {
      localStorage.setItem(key, mode);
      localStorage.setItem("hardened-blikvm-menu-auto-hide", "always");
      localStorage.setItem(
        "hardened-blikvm-upscale",
        JSON.stringify({ mode: upscale, sharpness: "medium" }),
      );
      if (renderer) {
        const getParameter = WebGLRenderingContext.prototype.getParameter;
        WebGLRenderingContext.prototype.getParameter = function (name) {
          // UNMASKED_RENDERER_WEBGL and RENDERER.
          if (name === 0x9246 || name === 0x1f01) return renderer;
          return getParameter.call(this, name);
        };
      }
      // The frames a VP8 decoder gives Chromium: the board's samples as they
      // are, which Chromium takes as of the narrow range with the BT.601
      // matrix; an H.264 stream of the fixed encoders says full range.
      const colorSpace =
        marked === "narrow"
          ? {
              primaries: "smpte170m",
              transfer: "smpte170m",
              matrix: "smpte170m",
              fullRange: false,
            }
          : {
              primaries: "bt709",
              transfer: "iec61966-2-1",
              matrix: "smpte170m",
              fullRange: true,
            };
      const rows = Math.ceil(patches.length / columns);
      const luma = new Uint8Array(width * height);
      const chroma = (width / 2) * (height / 2);
      const blue = new Uint8Array(chroma);
      const red = new Uint8Array(chroma);
      for (let row = 0; row < height; row += 1) {
        for (let column = 0; column < width; column += 1) {
          const patch =
            patches[
              Math.floor((row * rows) / height) * columns +
                Math.floor((column * columns) / width)
            ];
          luma[row * width + column] = patch[0];
          if (row % 2 === 0 && column % 2 === 0) {
            const at = (row / 2) * (width / 2) + column / 2;
            blue[at] = patch[1];
            red[at] = patch[2];
          }
        }
      }
      const samples = new Uint8Array(luma.length + 2 * chroma);
      samples.set(luma);
      samples.set(blue, luma.length);
      samples.set(red, luma.length + chroma);
      // A track of those frames, five a second, until its peer is closed:
      // SwiftShader draws an upscaled 2556x1438 buffer on the page's thread
      // at a cost no GPU has, and on a busy host kept up with ten only at
      // times (the watchdog then stopped upscaling); enough of five come
      // for the watchdog to judge a slowed draw.
      const picture = () => {
        const track = new MediaStreamTrackGenerator({ kind: "video" });
        const writer = track.writable.getWriter();
        let timestamp = 0;
        const timer = setInterval(() => {
          timestamp += 200000;
          const frame = new VideoFrame(samples, {
            format: "I420",
            codedWidth: width,
            codedHeight: height,
            timestamp,
            colorSpace,
          });
          writer.write(frame).catch(() => frame.close());
        }, 200);
        return {
          track,
          stop() {
            clearInterval(timer);
            track.stop();
          },
        };
      };
      class FixturePeer extends EventTarget {
        connectionState = "connected";
        onicecandidate = null;
        onconnectionstatechange = null;
        #ontrack = null;
        #picture = null;
        get ontrack() {
          return this.#ontrack;
        }
        set ontrack(handler) {
          this.#ontrack = handler;
          if (!handler) return;
          setTimeout(() => {
            this.#picture ??= picture();
            handler({ track: this.#picture.track });
          }, 0);
        }
        async getStats() {
          return new Map();
        }
        close() {
          this.#picture?.stop();
        }
      }
      Object.defineProperty(window, "RTCPeerConnection", {
        value: FixturePeer,
        configurable: true,
        writable: true,
      });
    },
    [
      MODE_KEY,
      mode,
      PATCHES,
      COLUMNS,
      WIDTH,
      HEIGHT,
      marked,
      renderer,
      upscale,
    ],
  );
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  // Where the input is watched: without the board's power control the page
  // sends it over its WebSocket at once, with no lease.
  await page.route("**/api/capabilities", (route) =>
    route.fulfill({
      json: {
        code: 0,
        data: sent
          ? {
              ...manifest,
              power: {
                ...manifest.power,
                status: { ...manifest.power.status, availability: "read_only" },
              },
            }
          : manifest,
      },
    }),
  );
  if (sent) {
    await page.routeWebSocket("**/api/ws", (socket) =>
      socket.onMessage((message) => {
        if (typeof message !== "string") sent.push([...message]);
      }),
    );
  }
  await page.route("**/api/system/firewall**", (route) =>
    route.fulfill({
      json: {
        code: 0,
        data: { effectiveMode: "baseline", webrtcBlocked: false },
      },
    }),
  );
  // The board's signalling: the picture flows.
  await page.routeWebSocket("**/api/stream/webrtc**", (socket) => {
    socket.send(JSON.stringify({ type: "started" }));
  });
  await page.goto(app.baseUrl, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => {
    // The video: the screen itself, or the source of the upscaled one.
    const video = document.querySelector(
      "video#screen, video[data-upscale-source]",
    );
    return video && video.videoWidth > 0 && video.readyState >= 2;
  });
  // The picture fills the view, so that every patch is large on the screen.
  await page.waitForTimeout(500);
  return { context, page, errors };
}

/** Each patch's mean red, green and blue at its centre, as drawn. */
async function drawn(page) {
  const box = await page.locator("#screen").boundingBox();
  assert.ok(box && box.width > 200 && box.height > 100, "no picture");
  const png = await page.screenshot({ clip: box });
  return page.evaluate(
    async ({ png, count, columns }) => {
      const bitmap = await createImageBitmap(
        new Blob([Uint8Array.from(atob(png), (c) => c.charCodeAt(0))], {
          type: "image/png",
        }),
      );
      const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
      const context = canvas.getContext("2d", { willReadFrequently: true });
      context.drawImage(bitmap, 0, 0);
      const rows = Math.ceil(count / columns);
      const width = bitmap.width / columns;
      const height = bitmap.height / rows;
      const levels = [];
      for (let index = 0; index < count; index += 1) {
        const x = Math.floor((index % columns) * width + width / 2);
        const y = Math.floor(Math.floor(index / columns) * height + height / 2);
        const data = context.getImageData(x - 8, y - 8, 16, 16).data;
        const sum = [0, 0, 0];
        for (let at = 0; at < data.length; at += 4) {
          for (let channel = 0; channel < 3; channel += 1)
            sum[channel] += data[at + channel];
        }
        levels.push(sum.map((value) => value / (data.length / 4)));
      }
      return levels;
    },
    {
      png: png.toString("base64"),
      count: PATCHES.length,
      columns: COLUMNS,
    },
  );
}

/** The largest difference of a drawn channel from the JPEG's drawing. */
function worst(levels, select) {
  let largest = 0;
  PATCHES.forEach((patch, index) => {
    if (!select(patch)) return;
    const want = jpeg(patch);
    for (let channel = 0; channel < 3; channel += 1) {
      largest = Math.max(
        largest,
        Math.abs(levels[index][channel] - want[channel]),
      );
    }
  });
  return largest;
}

const grey = (patch) => patch[1] === 128 && patch[2] === 128;
const kept = (patch) => grey(patch) && patch[0] > 16 && patch[0] < 235;
const colour = (patch) => !grey(patch);

const measured = {};
const round = (value) => Math.round(value * 10) / 10;

try {
  // VP8: drawn through the table, as the JPEG; what the browser made black
  // or white stays so.
  {
    const { context, page, errors } = await open("vp8", "narrow", GPU);
    const filter = await page.evaluate(
      () => getComputedStyle(document.getElementById("screen")).filter,
    );
    assert.equal(filter, 'url("#vp8-full-range")');
    const table = await page.evaluate(() =>
      [
        ...document.querySelectorAll("#vp8-full-range feComponentTransfer > *"),
      ].map(
        (node) =>
          `${node.tagName}:${node.getAttribute("type")}:${node.getAttribute("tableValues").split(" ").length}`,
      ),
    );
    assert.deepEqual(table, [
      "feFuncR:table:256",
      "feFuncG:table:256",
      "feFuncB:table:256",
    ]);
    const levels = await drawn(page);
    measured.vp8Greys = round(worst(levels, kept));
    measured.vp8Colours = round(worst(levels, colour));
    assert.ok(measured.vp8Greys <= 1.5, `greys ${measured.vp8Greys}`);
    assert.ok(measured.vp8Colours <= 4, `colours ${measured.vp8Colours}`);
    for (const [index, patch] of PATCHES.entries()) {
      if (!grey(patch)) continue;
      if (patch[0] <= 16)
        assert.deepEqual(levels[index], [0, 0, 0], `${patch}`);
      if (patch[0] >= 235)
        assert.deepEqual(levels[index], [255, 255, 255], `${patch}`);
    }
    // The same frames without the table: the browser's stretch, darker.
    await page.evaluate(() => {
      document.getElementById("screen").style.filter = "none";
    });
    await page.waitForTimeout(300);
    const stretched = await drawn(page);
    measured.stretched = round(worst(stretched, kept));
    assert.ok(
      measured.stretched > 10,
      "the fixture's frames were not stretched",
    );
    assert.ok(stretched[3][0] < 15, `27 drawn as ${stretched[3][0]}`);
    assert.deepEqual(errors, []);
    await context.close();
  }
  // Upscaled (docs/design/screen-upscaling-v1.md): on a display of pixel
  // ratio 2 the picture is shown twice its size, and with Auto a canvas draws
  // it through the upscaler. VP8 goes through the table in the shader, as
  // the JPEG, and H.264 is drawn unchanged, within the same tolerances.
  for (const [codec, marked] of [
    ["vp8", "narrow"],
    ["h264", "full"],
  ]) {
    const sent = [];
    const { context, page, errors } = await open(codec, marked, GPU, {
      upscale: "auto",
      ratio: 2,
      sent,
    });
    await page.waitForFunction(
      () =>
        document.querySelector("canvas#screen")?.dataset.upscale === "medium",
    );
    // The buffer has the picture's device pixels: its CSS size times 2.
    const buffer = await page.evaluate(() => {
      const canvas = document.querySelector("canvas#screen");
      const box = canvas.getBoundingClientRect();
      return {
        reported: canvas.dataset.upscaleBuffer,
        shown: `${Math.round(box.width * 2)}x${Math.round(box.height * 2)}`,
      };
    });
    assert.equal(buffer.reported, buffer.shown);
    // The watchdog's count, as the monitor shows it: the video's frames
    // drawn through the upscaler, each of them.
    await page.waitForTimeout(3000);
    const [drawnFrames, cameFrames] = await page.evaluate(() =>
      document
        .querySelector("canvas#screen")
        .dataset.upscaleFrames.split("/")
        .map(Number),
    );
    assert.ok(
      cameFrames >= 3 && drawnFrames >= cameFrames * 0.9,
      `drawn ${drawnFrames} of ${cameFrames} a second`,
    );
    // No SVG filter is applied to anything: the shader has the table.
    assert.equal(
      await page.evaluate(() =>
        [...document.querySelectorAll("#screen, video")].some(
          (element) => getComputedStyle(element).filter !== "none",
        ),
      ),
      false,
    );
    const levels = await drawn(page);
    if (codec === "vp8") {
      measured.upscaledVp8Greys = round(worst(levels, kept));
      measured.upscaledVp8Colours = round(worst(levels, colour));
      assert.ok(
        measured.upscaledVp8Greys <= 1.5,
        `greys ${measured.upscaledVp8Greys}`,
      );
      assert.ok(
        measured.upscaledVp8Colours <= 4,
        `colours ${measured.upscaledVp8Colours}`,
      );
      for (const [index, patch] of PATCHES.entries()) {
        if (!grey(patch)) continue;
        if (patch[0] <= 16)
          assert.deepEqual(levels[index], [0, 0, 0], `${patch}`);
        if (patch[0] >= 235)
          assert.deepEqual(levels[index], [255, 255, 255], `${patch}`);
      }
      // The pointer lands on the picture where it is: the canvas is the
      // screen now, and the mouse bound to it.
      const box = await page.locator("canvas#screen").boundingBox();
      // A mouse event's position is in whole pixels.
      const [x, y] = [Math.round(box.x) + 300, Math.round(box.y) + 200];
      await page.mouse.move(x, y);
      let report = null;
      for (let attempt = 0; attempt < 40 && !report; attempt += 1) {
        report = sent.filter((data) => data[0] === 2).at(-1) ?? null;
        if (!report) await page.waitForTimeout(50);
      }
      assert.ok(report, "no mouse report sent");
      const at = (low, high) => report[low] | (report[high] << 8);
      // Within two of 32767: the layout's fractions of a pixel.
      const near = (got, want) =>
        assert.ok(Math.abs(got - want) <= 2, `${got} for ${want}`);
      near(at(2, 3), Math.round((0x7fff * (x - box.x)) / box.width));
      near(at(4, 5), Math.round((0x7fff * (y - box.y)) / box.height));
    } else {
      measured.upscaledH264 = round(
        Math.max(worst(levels, grey), worst(levels, colour)),
      );
      assert.ok(measured.upscaledH264 <= 1.5, `H.264 ${measured.upscaledH264}`);
    }
    assert.deepEqual(errors, []);
    await context.close();
  }
  // The watchdog: a drawing that falls behind lets the video present
  // frames the canvas never draws (the video counts them itself). Each frame
  // callback is handed over 300 ms late, as a slow draw would leave it, the
  // page's thread itself left free: headless Chromium, which presents video
  // with that thread, would slow the video with it and hide the loss. Within
  // seconds upscaling stops for the page load with a notice, and the video
  // is the screen again and plays.
  {
    const { context, page, errors } = await open("vp8", "narrow", GPU, {
      upscale: "auto",
      ratio: 2,
    });
    await page.waitForFunction(
      () =>
        document.querySelector("canvas#screen")?.dataset.upscale === "medium",
    );
    await page.evaluate(() => {
      const request = HTMLVideoElement.prototype.requestVideoFrameCallback;
      HTMLVideoElement.prototype.requestVideoFrameCallback = function (back) {
        return request.call(this, (now, metadata) =>
          setTimeout(() => back(now, metadata), 300),
        );
      };
    });
    const slowed = Date.now();
    await page.getByTestId("upscale-notice").waitFor({ timeout: 15000 });
    measured.watchdog = round((Date.now() - slowed) / 1000);
    await page.waitForFunction(
      () =>
        document.querySelector("video#screen") &&
        !document.querySelector("canvas#screen"),
    );
    const playedAt = () =>
      page.evaluate(() => document.querySelector("video#screen").currentTime);
    const before = await playedAt();
    await page.waitForTimeout(500);
    assert.ok((await playedAt()) > before, "the video stands still");
    assert.deepEqual(errors, []);
    await context.close();
  }
  // H.264 in the same view says what its samples mean: drawn unchanged.
  {
    const { context, page, errors } = await open("h264", "full", GPU);
    assert.equal(
      await page.evaluate(
        () => getComputedStyle(document.getElementById("screen")).filter,
      ),
      "none",
    );
    assert.equal(await page.locator("#vp8-full-range").count(), 0);
    const levels = await drawn(page);
    measured.h264 = round(Math.max(worst(levels, grey), worst(levels, colour)));
    assert.ok(measured.h264 <= 1.5, `H.264 ${measured.h264}`);
    assert.deepEqual(errors, []);
    await context.close();
  }
  // Drawn in software: VP8 as before, without the filter, and not
  // upscaled although upscaling is Auto: the video is the screen.
  {
    const { context, page, errors } = await open("vp8", "narrow", null, {
      upscale: "auto",
      ratio: 2,
    });
    assert.match(
      await page.evaluate(() => {
        const context = document.createElement("canvas").getContext("webgl");
        const info = context?.getExtension("WEBGL_debug_renderer_info");
        return String(
          info && context.getParameter(info.UNMASKED_RENDERER_WEBGL),
        );
      }),
      /SwiftShader/,
      "this Chromium draws with a GPU",
    );
    assert.equal(
      await page.evaluate(
        () => getComputedStyle(document.getElementById("screen")).filter,
      ),
      "none",
    );
    assert.equal(await page.locator("#vp8-full-range").count(), 0);
    await page.waitForTimeout(1000);
    assert.equal(await page.locator("canvas#screen").count(), 0);
    assert.equal(await page.locator("video#screen").count(), 1);
    const levels = await drawn(page);
    assert.ok(worst(levels, kept) > 10, "drawn otherwise than before");
    assert.deepEqual(errors, []);
    await context.close();
  }
  // A browser that names a GPU but has no WebGL2 (Chromium told so): VP8
  // through the table on the video as before, and with Auto nothing to
  // upscale with: no canvas and no notice.
  {
    const withoutWebgl2 = await launchChromium({ args: ["--disable-webgl2"] });
    try {
      const { context, page, errors } = await open("vp8", "narrow", GPU, {
        upscale: "auto",
        ratio: 2,
        from: withoutWebgl2,
      });
      assert.equal(
        await page.evaluate(
          () => document.createElement("canvas").getContext("webgl2") !== null,
        ),
        false,
      );
      await page.waitForTimeout(1000);
      assert.equal(await page.locator("canvas#screen").count(), 0);
      assert.equal(await page.locator("video#screen").count(), 1);
      assert.equal(
        await page.evaluate(
          () => getComputedStyle(document.getElementById("screen")).filter,
        ),
        'url("#vp8-full-range")',
      );
      assert.equal(await page.getByTestId("upscale-notice").count(), 0);
      const levels = await drawn(page);
      measured.withoutWebgl2 = round(worst(levels, kept));
      assert.ok(
        measured.withoutWebgl2 <= 1.5,
        `greys ${measured.withoutWebgl2}`,
      );
      assert.deepEqual(errors, []);
      await context.close();
    } finally {
      await withoutWebgl2.close();
    }
  }
  console.log(
    `VP8 range: the board's full-range samples drawn as the JPEG in VP8 (greys within ${measured.vp8Greys}, colours within ${measured.vp8Colours} levels, black and white kept; ${measured.stretched} without the table), and H.264 over the same view drawn unchanged (within ${measured.h264}); upscaled, VP8's greys within ${measured.upscaledVp8Greys} and colours within ${measured.upscaledVp8Colours}, H.264 within ${measured.upscaledH264}; a slowed draw stopped by the watchdog after ${measured.watchdog} s; a browser drawing in software left as before, not upscaled, and one without WebGL2 drawing through the table as before (greys within ${measured.withoutWebgl2}), not upscaled, passed`,
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
