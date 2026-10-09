// The MJPEG picture through the page's upscaler (docs/design/screen-upscaling-v1.md), through the built page with a
// played stream. The board's stream is a multipart response of JPEG frames: a picture of flat patches and of black
// and white squares on the 8x8 grid of the JPEG's blocks. On a display of pixel ratio 2 the picture is shown twice
// its size. Plainly the browser scales the <img>; with Auto the page reads the stream itself and draws its frames
// through the upscaler into a canvas of the shown size. The patches come out at the plain image's levels, the
// squares' edges rise at least 15 % more narrowly than the browser's own scaling of the image (which is better than
// bilinear), the frames read are drawn, and the mouse lands where it is on the picture.
import assert from "node:assert/strict";
import { defaultManifest, startApp } from "./app_harness.mjs";
import { launchChromium } from "./quality_fixture.mjs";

const browser = await launchChromium();
const app = await startApp();
const GPU =
  "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 (0x00002503) Direct3D11 vs_5_0 ps_5_0, D3D11)";
const WIDTH = 640;
const HEIGHT = 360;
const FRAMES = 6;
// Levels of the flat patches, and where the squares are.
const PATCHES = [16, 48, 96, 128, 160, 200, 235];
const SQUARES_Y = 200;

/** JPEG frames of the test picture, made by the browser's encoder at quality 0.75. */
async function makeFrames() {
  const page = await browser.newPage();
  const frames = await page.evaluate(
    async ({ width, height, patches, squaresY, count }) => {
      const out = [];
      for (let index = 0; index < count; index += 1) {
        const canvas = new OffscreenCanvas(width, height);
        const context = canvas.getContext("2d");
        context.fillStyle = "#808080";
        context.fillRect(0, 0, width, height);
        patches.forEach((level, at) => {
          context.fillStyle = `rgb(${level},${level},${level})`;
          context.fillRect(16 + at * 88, 16, 80, 80);
        });
        // Squares of 32 pixels on the blocks' grid, black and white in turn.
        for (let at = 0; at < 16; at += 1) {
          context.fillStyle = at % 2 ? "#ffffff" : "#000000";
          context.fillRect(32 + at * 32, squaresY, 32, 64);
        }
        // A mark that changes with the frame.
        context.fillStyle = "#ffffff";
        context.fillRect(16 + index * 8, 320, 8, 8);
        const blob = await canvas.convertToBlob({
          type: "image/jpeg",
          quality: 0.75,
        });
        out.push([...new Uint8Array(await blob.arrayBuffer())]);
      }
      return out;
    },
    {
      width: WIDTH,
      height: HEIGHT,
      patches: PATCHES,
      squaresY: SQUARES_Y,
      count: FRAMES,
    },
  );
  await page.close();
  return frames.map((bytes) => Buffer.from(bytes));
}

const frames = await makeFrames();
const stream = Buffer.concat(
  frames.flatMap((jpeg) => [
    Buffer.from(
      `--frame\r\nContent-Type: image/jpeg\r\nContent-Length: ${jpeg.length}\r\n\r\n`,
    ),
    jpeg,
    Buffer.from("\r\n"),
  ]),
);

async function open(upscale, sent = null) {
  const context = await browser.newContext({
    viewport: { width: 1280, height: 720 },
    deviceScaleFactor: 2,
  });
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  await context.addInitScript(
    ([gpu, upscale]) => {
      localStorage.setItem("hardened-blikvm-menu-auto-hide", "always");
      localStorage.setItem(
        "hardened-blikvm-upscale",
        JSON.stringify({ mode: upscale, sharpness: "medium" }),
      );
      const getParameter = WebGLRenderingContext.prototype.getParameter;
      WebGLRenderingContext.prototype.getParameter = function (name) {
        if (name === 0x9246 || name === 0x1f01) return gpu;
        return getParameter.call(this, name);
      };
    },
    [GPU, upscale],
  );
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  // Without the board's power control the page sends input over its
  // WebSocket at once, with no lease.
  const manifest = defaultManifest();
  await page.route("**/api/capabilities", (route) =>
    route.fulfill({
      json: {
        code: 0,
        data: {
          ...manifest,
          power: {
            ...manifest.power,
            status: { ...manifest.power.status, availability: "read_only" },
          },
        },
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
  await page.route("**/api/stream/mjpeg**", (route) =>
    route.fulfill({
      status: 200,
      headers: {
        "content-type": "multipart/x-mixed-replace; boundary=frame",
      },
      body: stream,
    }),
  );
  await page.goto(app.baseUrl, { waitUntil: "domcontentloaded" });
  await page.locator("#screen").waitFor();
  return { context, page, errors };
}

/** Grey levels of the patches' middles and the squares' edge rise, from a screenshot of the screen. */
async function measure(page) {
  const box = await page.locator("#screen").boundingBox();
  const png = await page.screenshot({ clip: box });
  return page.evaluate(
    async ({ base64, patches, squaresY, width }) => {
      const image = await createImageBitmap(
        new Blob([Uint8Array.from(atob(base64), (one) => one.charCodeAt(0))], {
          type: "image/png",
        }),
      );
      const scale = image.width / width;
      const canvas = new OffscreenCanvas(image.width, image.height);
      const context = canvas.getContext("2d", { willReadFrequently: true });
      context.drawImage(image, 0, 0);
      const { data } = context.getImageData(0, 0, image.width, image.height);
      const light = (x, y) => {
        const at = 4 * (Math.round(y) * image.width + Math.round(x));
        return (data[at] + data[at + 1] + data[at + 2]) / 3;
      };
      const levels = patches.map((_, at) => {
        let sum = 0;
        for (let dy = -8; dy < 8; dy += 1) {
          for (let dx = -8; dx < 8; dx += 1) {
            sum += light((56 + at * 88) * scale + dx, 56 * scale + dy);
          }
        }
        return sum / 256;
      });
      // Across each boundary between two squares, over their middle rows.
      const rises = [];
      for (let at = 1; at < 16; at += 1) {
        const edge = (32 + at * 32) * scale;
        const profile = [];
        for (let dx = -10 * scale; dx < 10 * scale; dx += 1) {
          let sum = 0;
          for (
            let y = (squaresY + 16) * scale;
            y < (squaresY + 48) * scale;
            y += 1
          ) {
            sum += light(edge + dx, y);
          }
          profile.push(sum / (32 * scale));
        }
        const low = profile[0];
        const high = profile[profile.length - 1];
        const crossing = (share) => {
          const wanted = low + (high - low) * share;
          for (let index = 1; index < profile.length; index += 1) {
            const [a, b] = [profile[index - 1], profile[index]];
            if ((a - wanted) * (b - wanted) <= 0 && a !== b) {
              return index - 1 + (wanted - a) / (b - a);
            }
          }
          return null;
        };
        const [from, to] = [crossing(0.1), crossing(0.9)];
        if (from !== null && to !== null)
          rises.push(Math.abs(to - from) / scale);
      }
      return {
        levels,
        rise: rises.reduce((sum, value) => sum + value, 0) / rises.length,
      };
    },
    {
      base64: png.toString("base64"),
      patches: PATCHES,
      squaresY: SQUARES_Y,
      width: WIDTH,
    },
  );
}

try {
  // Plainly: the <img> of the stream, stretched by the browser.
  const plain = await (async () => {
    const { context, page, errors } = await open("off");
    await page.waitForFunction(() => {
      const image = document.querySelector("img#screen");
      return image && image.naturalWidth > 0;
    });
    await page.waitForTimeout(300);
    const seen = await measure(page);
    assert.deepEqual(errors, []);
    await context.close();
    return seen;
  })();

  // Upscaled: the page reads the stream and draws its frames.
  const sent = [];
  const { context, page, errors } = await open("auto", sent);
  // Upscaled, and drawn at the size the picture is laid out at once the
  // stream has said its size (the last frame is drawn again for it).
  await page.waitForFunction(() => {
    const canvas = document.querySelector("canvas#screen");
    if (canvas?.dataset.upscale !== "medium") return false;
    const box = canvas.getBoundingClientRect();
    return (
      canvas.dataset.upscaleBuffer ===
        `${Math.round(box.width * 2)}x${Math.round(box.height * 2)}` &&
      canvas.dataset.pictureWidth === "640"
    );
  });
  const state = await page.evaluate(() => {
    const canvas = document.querySelector("canvas#screen");
    const box = canvas.getBoundingClientRect();
    return {
      buffer: canvas.dataset.upscaleBuffer,
      shown: `${Math.round(box.width * 2)}x${Math.round(box.height * 2)}`,
      picture: `${canvas.dataset.pictureWidth}x${canvas.dataset.pictureHeight}`,
      frames: canvas.dataset.mjpegFrames,
    };
  });
  assert.equal(state.buffer, state.shown);
  assert.equal(state.picture, `${WIDTH}x${HEIGHT}`);
  const [received, decoded, drawn] = state.frames.split("/").map(Number);
  assert.ok(received >= FRAMES && decoded >= 1 && drawn >= 1, state.frames);

  const upscaled = await measure(page);
  const worst = Math.max(
    ...upscaled.levels.map((level, at) => Math.abs(level - plain.levels[at])),
  );
  assert.ok(worst <= 2, `patches ${upscaled.levels} against ${plain.levels}`);
  // The browser scales an image better than bilinearly (the gate's 25 % is
  // against bilinear): at least 15 % narrower than the image as it is drawn.
  assert.ok(
    upscaled.rise <= plain.rise * 0.85,
    `the squares' edges rise in ${upscaled.rise.toFixed(2)} px upscaled, ${plain.rise.toFixed(2)} px stretched`,
  );

  // The pointer lands on the picture where it is.
  const box = await page.locator("canvas#screen").boundingBox();
  const [x, y] = [Math.round(box.x) + 211, Math.round(box.y) + 157];
  await page.mouse.move(x, y);
  let report = null;
  for (let attempt = 0; attempt < 40 && !report; attempt += 1) {
    report = sent.filter((data) => data[0] === 2).at(-1) ?? null;
    if (!report) await page.waitForTimeout(50);
  }
  assert.ok(report, "no mouse report sent");
  const at = (low, high) => report[low] | (report[high] << 8);
  const near = (got, want) =>
    assert.ok(Math.abs(got - want) <= 2, `${got} for ${want}`);
  near(at(2, 3), Math.round((0x7fff * (x - box.x)) / box.width));
  near(at(4, 5), Math.round((0x7fff * (y - box.y)) / box.height));
  assert.deepEqual(errors, []);
  await context.close();

  console.log(
    `MJPEG upscaled: the stream read and its frames drawn (${state.frames} received/decoded/drawn) into a ${state.buffer} canvas, the patches within ${worst.toFixed(1)} of the plain image, the squares' edges rising in ${upscaled.rise.toFixed(2)} px against ${plain.rise.toFixed(2)} px stretched, the pointer where it is passed`,
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
