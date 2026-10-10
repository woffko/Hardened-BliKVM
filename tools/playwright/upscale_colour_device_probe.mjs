// On a board: the colour of the picture with upscaling on
// (docs/design/screen-upscaling-v1.md, "The views"), against the MJPEG
// picture. For the enlargements 133 % and 200 %, each way the picture comes
// (MJPEG, VP8 and H.264 over WebRTC, H.264 over QUIC) is shown at once in
// browsers of their own, plainly and upscaled, beside the MJPEG picture shown
// plainly, and the three are read at the same moment as a grid of patches,
// each patch's mean colour taken from its middle. The board serves two MJPEG
// streams at once: the MJPEG picture's browser is also the plain MJPEG view,
// and the other two start on H.264. The three must be laid out alike. Where the target's screen changed between the reads, the plain and
// the upscaled picture differ in more than a tenth of the patches, and the
// view is shown and read again. A view passes where, over the patches that
// show the same, its upscaled picture is on each channel within one level of
// its plain one, as a whole and in each of the dark, middle and light tones
// that at least four patches have: upscaling sharpens edges and changes no
// colour, so the upscaled picture is as far from the MJPEG one as the plain
// picture is. How far each is from the MJPEG picture is recorded.
//
// Chromium draws in software here. Each browser is given a GPU's renderer
// name, so that the page draws as for a GPU (VP8 through its range table)
// and Auto upscales (in SwiftShader). The upscaled view is read at once:
// SwiftShader draws a large buffer more slowly than the board sends frames,
// and the watchdog, which judges a view after its first five seconds, would
// stop upscaling. The probe logs in once in each browser and watches: it asks
// for no lease and sends no input (any /api/hid request fails it), and it
// changes nothing on the board; the browsers' choices are set in their own
// storage. The GUI password is read from stdin; a receipt is written under
// evidence/private, without tokens. No screenshot is kept.
//
// Usage: node upscale_colour_device_probe.mjs BASE_URL USERNAME OUTPUT [ROUNDS]
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [baseUrl, username, outputValue, roundsValue] = process.argv.slice(2);
const board = new URL(baseUrl);
assert.equal(board.protocol, "https:");
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
/** Attempts a view may take before its pictures are read showing the same. */
const ROUNDS = Number(roundsValue ?? 6);
assert.ok(ROUNDS >= 1 && ROUNDS <= 20);
const ENLARGEMENTS = [4 / 3, 2];
const MJPEG = { name: "mjpeg", mode: "mjpeg", transport: "standard" };
const VIEWS = [
  MJPEG,
  { name: "vp8", mode: "vp8", transport: "standard" },
  { name: "h264-webrtc", mode: "h264", transport: "standard" },
  { name: "h264-quic", mode: "h264", transport: "quic" },
];
/** The grid the visible part is read in, and the share of a patch read. */
const COLUMNS = 12;
const ROWS = 8;
const MIDDLE = 0.6;
/**
 * A patch shows the same in two pictures where each channel is within this
 * many levels, and two pictures show the same where this share of patches
 * do.
 */
const SAME = 6;
const SAME_SHARE = 0.9;
/** A view's upscaled patches within this of its plain ones, on average. */
const WITHIN = 1;
/** The tones by the plain patch's luma, and the patches a tone is judged on. */
const TONES = [
  ["dark", 0, 64],
  ["middle", 64, 192],
  ["light", 192, 256],
];
const TONE_PATCHES = 4;
const GPU =
  "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 (0x00002503) Direct3D11 vs_5_0 ps_5_0, D3D11)";

let password = "";
for await (const chunk of process.stdin) password += chunk.toString("utf8");
password = password.replace(/[\r\n]+$/, "");
assert.ok(password.length >= 8);

const { chromium } = await import(
  pathToFileURL(
    process.env.PLAYWRIGHT_MODULE ??
      path.join(
        os.homedir(),
        ".local/share/playwright/node_modules/playwright/index.mjs",
      ),
  ).href
);
const browser = await chromium.launch({
  headless: true,
  executablePath:
    process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ??
    path.join(
      os.homedir(),
      ".cache/ms-playwright/chromium-1228/chrome-linux64/chrome",
    ),
});

const PATIENCE = 40000;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const round = (value, places = 2) =>
  Math.round(value * 10 ** places) / 10 ** places;

const receipt = {
  schemaVersion: 2,
  probe: "upscale-colour-device",
  browser: browser.version(),
  grid: { columns: COLUMNS, rows: ROWS, middle: MIDDLE },
  same: { levels: SAME, share: SAME_SHARE },
  enlargements: [],
  hidRequests: {},
  errors: [],
};
let result = "failed";

/** Whether the view shows its picture, plainly or upscaled: run in the page. */
function ready([view, upscaled]) {
  const screen = document.getElementById("screen");
  if (!screen) return false;
  if (view.transport === "quic") {
    const canvas = document.querySelector(
      'canvas#screen[data-video-transport="quic"]',
    );
    return (
      canvas !== null &&
      Number(canvas.dataset.quicFrames) >= 2 &&
      canvas.dataset.quicUpscale === (upscaled ? "medium" : "off")
    );
  }
  if (upscaled) {
    return screen.tagName === "CANVAS" && screen.dataset.upscale === "medium";
  }
  if (view.mode === "mjpeg") {
    return screen.tagName === "IMG" && screen.naturalWidth > 0;
  }
  return screen.tagName === "VIDEO" && screen.videoWidth > 0;
}

/** The median of the values. */
const median = (values) => {
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2
    ? sorted[middle]
    : (sorted[middle - 1] + sorted[middle]) / 2;
};
const mean = (values) =>
  values.reduce((sum, value) => sum + value, 0) / Math.max(1, values.length);
const luma = ([red, green, blue]) => 0.299 * red + 0.587 * green + 0.114 * blue;

/** Whether two pictures are laid out at the same place and size. */
const alike = (one, other) =>
  ["x", "y", "width", "height"].every(
    (key) => Math.abs(one[key] - other[key]) <= 1,
  );
const where = (box) =>
  `${round(box.x, 1)},${round(box.y, 1)} ${round(box.width, 1)}x${round(box.height, 1)}`;

/** The patches that show the same in both pictures. */
const sameIn = (of, from) =>
  of
    .map((patch, index) => index)
    .filter((index) =>
      [0, 1, 2].every(
        (channel) =>
          Math.abs(of[index][channel] - from[index][channel]) <= SAME,
      ),
    );

/** `of` minus `from` on each channel, on average over the patches. */
const apart = (of, from, patches) =>
  [0, 1, 2].map((channel) =>
    round(
      mean(patches.map((index) => of[index][channel] - from[index][channel])),
    ),
  );

try {
  const origin = board.origin;
  /** A browser of its own, logged in, with its page showing `first` plainly. */
  async function browse(first) {
    const context = await browser.newContext({
      ignoreHTTPSErrors: true,
      locale: "en-US",
      viewport: { width: 1440, height: 1000 },
    });
    await context.addInitScript(
      ([gpu, first]) => {
        const getParameter = WebGLRenderingContext.prototype.getParameter;
        WebGLRenderingContext.prototype.getParameter = function (name) {
          // UNMASKED_RENDERER_WEBGL and RENDERER.
          if (name === 0x9246 || name === 0x1f01) return gpu;
          return getParameter.call(this, name);
        };
        if (!location.protocol.startsWith("http")) return;
        if (localStorage.getItem("hardened-blikvm-video-mode") !== null) return;
        localStorage.setItem("hardened-blikvm-video-mode", first.mode);
        localStorage.setItem("hardened-blikvm-transport", first.transport);
        localStorage.setItem(
          "hardened-blikvm-upscale",
          JSON.stringify({
            mode: "off",
            sharpness: "medium",
            size: "fill",
            target: null,
          }),
        );
      },
      [GPU, first],
    );
    const login = await context.request.post(`${origin}/api/auth/login`, {
      data: { username, password },
      headers: { Origin: origin },
    });
    assert.ok((await login.json()).data?.csrfToken, "login failed");
    const page = await context.newPage();
    page.setDefaultTimeout(PATIENCE);
    page.on("pageerror", (error) =>
      receipt.errors.push(String(error.message).slice(0, 200)),
    );
    page.on("request", (request) => {
      const route = new URL(request.url()).pathname;
      if (route.startsWith("/api/hid")) {
        const key = `${request.method()} ${route}`;
        receipt.hidRequests[key] = (receipt.hidRequests[key] ?? 0) + 1;
      }
    });
    await page.goto(origin, { waitUntil: "load" });
    return page;
  }
  const H264 = VIEWS[2];
  const referencePage = await browse(MJPEG);
  const plainPage = await browse(H264);
  const upscaledPage = await browse(H264);
  password = "";

  /** Shows the view as the browser's choices say, and waits for its picture. */
  async function show(page, view, upscaled, zoom) {
    await page.evaluate(
      ([view, upscaled, zoom]) => {
        localStorage.setItem("hardened-blikvm-video-mode", view.mode);
        localStorage.setItem("hardened-blikvm-transport", view.transport);
        localStorage.setItem("hardened-blikvm-menu-auto-hide", "always");
        localStorage.setItem("hardened-blikvm-video-scale", String(zoom));
        localStorage.setItem(
          "hardened-blikvm-upscale",
          JSON.stringify({
            mode: upscaled ? "auto" : "off",
            sharpness: "medium",
            size: "fill",
            target: null,
          }),
        );
      },
      [view, upscaled, zoom],
    );
    await page.reload({ waitUntil: "load" });
    await page.waitForFunction(ready, [view, upscaled], {
      timeout: PATIENCE,
      polling: 200,
    });
    // A frame or two more, drawn at the size it settled at; an upscaled
    // view is read at once.
    await sleep(upscaled ? 300 : 1500);
    return page.evaluate(() => {
      const screen = document.getElementById("screen");
      const box = screen.getBoundingClientRect();
      const width = Number(screen.dataset.pictureWidth) || 1920;
      return { width: box.width, picture: width };
    });
  }

  /**
   * Where the screen is laid out, and the visible part of it as a grid of
   * patches' mean colours, read while the view still shows what it was
   * shown for; `null` where it no longer does (it was mounted again, or
   * upscaling stopped).
   */
  async function patches(page, view, upscaled) {
    if (!(await page.evaluate(ready, [view, upscaled]))) return null;
    const box = await page.locator("#screen").boundingBox();
    if (!box) return null;
    const viewport = page.viewportSize();
    const clip = {
      x: Math.max(0, box.x),
      y: Math.max(0, box.y),
      width: Math.min(box.x + box.width, viewport.width) - Math.max(0, box.x),
      height:
        Math.min(box.y + box.height, viewport.height) - Math.max(0, box.y),
    };
    const png = await page.screenshot({ clip });
    if (!(await page.evaluate(ready, [view, upscaled]))) return null;
    const means = await page.evaluate(
      async ({ base64, columns, rows, middle }) => {
        const image = await createImageBitmap(
          new Blob(
            [Uint8Array.from(atob(base64), (one) => one.charCodeAt(0))],
            { type: "image/png" },
          ),
        );
        const canvas = new OffscreenCanvas(image.width, image.height);
        const context = canvas.getContext("2d", { willReadFrequently: true });
        context.drawImage(image, 0, 0);
        const { data } = context.getImageData(0, 0, image.width, image.height);
        const means = [];
        for (let row = 0; row < rows; row += 1) {
          for (let column = 0; column < columns; column += 1) {
            const width = image.width / columns;
            const height = image.height / rows;
            const x0 = Math.floor(column * width + (width * (1 - middle)) / 2);
            const y0 = Math.floor(row * height + (height * (1 - middle)) / 2);
            const x1 = Math.floor(x0 + width * middle);
            const y1 = Math.floor(y0 + height * middle);
            const sum = [0, 0, 0];
            let count = 0;
            for (let y = y0; y < y1; y += 1) {
              for (let x = x0; x < x1; x += 1) {
                const at = (y * image.width + x) * 4;
                sum[0] += data[at];
                sum[1] += data[at + 1];
                sum[2] += data[at + 2];
                count += 1;
              }
            }
            means.push(sum.map((value) => value / Math.max(1, count)));
          }
        }
        return means;
      },
      {
        base64: png.toString("base64"),
        columns: COLUMNS,
        rows: ROWS,
        middle: MIDDLE,
      },
    );
    return { box, means };
  }

  /**
   * An upscaled canvas not yet drawn: black, its patches at most a few levels
   * on average, where the plain picture is not.
   */
  const blank = (upscaled, plain) => {
    const level = (patches) => mean(patches.map((patch) => Math.max(...patch)));
    return level(upscaled) < 4 && level(plain) > 20;
  };

  // The fit of the picture in the view: the zoom that shows it at each
  // enlargement.
  const fitted = await show(referencePage, MJPEG, false, 1);
  const fit = fitted.width / fitted.picture;
  receipt.fit = round(fit, 4);

  for (const enlargement of ENLARGEMENTS) {
    const zoom = enlargement / fit;
    const measured = { enlargement: round(enlargement, 3), views: [] };
    const reference = await show(referencePage, MJPEG, false, zoom);
    for (const view of VIEWS) {
      let found = null;
      const lost = [];
      for (let attempt = 1; attempt <= ROUNDS && !found; attempt += 1) {
        const plain =
          view === MJPEG ? reference : await show(plainPage, view, false, zoom);
        const upscaled = await show(upscaledPage, view, true, zoom);
        // The three at once, the upscaled view before the watchdog may judge
        // it. SwiftShader draws a large buffer a few times a second: one not
        // yet drawn since it was made is all black, and is read again.
        let reads = [];
        let blanks = 0;
        for (let reading = 0; reading < 4; reading += 1) {
          reads = await Promise.all([
            patches(referencePage, MJPEG, false),
            view === MJPEG ? null : patches(plainPage, view, false),
            patches(upscaledPage, view, true),
          ]);
          if (view === MJPEG) reads[1] = reads[0];
          if (reads.some((one) => !one)) break;
          if (!blank(reads[2].means, reads[1].means)) break;
          blanks += 1;
          await sleep(250);
        }
        const [mjpeg, left, right] = reads;
        if (!mjpeg || !left || !right || blank(right.means, left.means)) {
          lost.push(
            `${attempt}${mjpeg ? "" : " mjpeg"}${left ? "" : " plain"}${right ? "" : " upscaled"}${blanks ? ` blank ${blanks}` : ""}`,
          );
          continue;
        }
        for (const [name, one] of [
          ["plain", left],
          ["MJPEG", mjpeg],
        ]) {
          assert.ok(
            alike(one.box, right.box),
            `${view.name} at ${round(enlargement, 3)}: the ${name} picture laid out at ${where(one.box)}, the upscaled one at ${where(right.box)}`,
          );
        }
        const same = sameIn(right.means, left.means);
        if (same.length < SAME_SHARE * right.means.length) {
          lost.push(`${attempt} changed ${right.means.length - same.length}`);
          continue;
        }
        const tones = TONES.map(([tone, low, high]) => {
          const inTone = same.filter((index) => {
            const level = luma(left.means[index]);
            return level >= low && level < high;
          });
          return {
            tone,
            patches: inTone.length,
            upscaledFromPlain: apart(right.means, left.means, inTone),
          };
        });
        const fromPlain = apart(right.means, left.means, same);
        const judged = [
          fromPlain,
          ...tones
            .filter((tone) => tone.patches >= TONE_PATCHES)
            .map((tone) => tone.upscaledFromPlain),
        ];
        // How far each is from the MJPEG picture, in the median patch: the
        // MJPEG picture is read in its own browser, and a frame apart where
        // the screen changes.
        const fromMjpeg = (of) =>
          [0, 1, 2].map((channel) =>
            round(
              median(
                same.map(
                  (index) => of[index][channel] - mjpeg.means[index][channel],
                ),
              ),
            ),
          );
        found = {
          view: view.name,
          attempt,
          lost,
          enlargement: {
            plain: round(plain.width / plain.picture, 3),
            upscaled: round(upscaled.width / upscaled.picture, 3),
          },
          laidOut: where(right.box),
          samePatches: same.length,
          upscaledFromPlain: fromPlain,
          tones,
          plainFromMjpeg: fromMjpeg(left.means),
          upscaledFromMjpeg: fromMjpeg(right.means),
          within: judged.every((channels) =>
            channels.every((value) => Math.abs(value) <= WITHIN),
          ),
        };
      }
      assert.ok(
        found,
        `${view.name} at ${round(enlargement, 3)}: not read showing the same in ${lost.join(", ")}`,
      );
      measured.views.push(found);
    }
    receipt.enlargements.push(measured);
  }
  for (const measured of receipt.enlargements) {
    for (const view of measured.views) {
      assert.ok(
        view.within,
        `${view.view} at ${measured.enlargement}: upscaled ${view.upscaledFromPlain} from plain, by tone ${view.tones.map((tone) => `${tone.tone} ${tone.patches}: ${tone.upscaledFromPlain}`).join("; ")}`,
      );
    }
  }
  assert.deepEqual(receipt.hidRequests, {}, "a HID request was made");
  assert.deepEqual(receipt.errors, []);
  result = "passed";
} catch (error) {
  receipt.error = String(error?.stack ?? error).slice(0, 2000);
} finally {
  receipt.result = result;
  fs.writeFileSync(output, `${JSON.stringify(receipt, null, 2)}\n`, {
    mode: 0o600,
  });
  await browser.close();
  console.log(`result: ${result}`);
  if (receipt.error) console.log(receipt.error.split("\n")[0]);
  process.exit(result === "passed" ? 0 : 1);
}
