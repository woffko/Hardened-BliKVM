// On a board: the H.264 engine chosen from the video menu while this page
// watches H.264 (docs/design/h264-engine-choice-and-quality-monitor-v1.md,
// gate C5). ROUNDS times each way, H.264 SW and then H.264 HW is clicked in
// the menu; the probe notes when GET /api/vm/screen says that x264 or the
// video engine encodes, and when the quality monitor (open in Basic) shows
// it, and that the picture goes on. At the end the board's choice is the one
// the probe found, whatever happens.
//
// The probe logs in once and watches. It asks for no lease and no
// confirmation of the target: any /api/hid request fails it. The transport
// and the mode are set in this browser's own storage. The GUI password is
// read from stdin; a receipt is written under evidence/private, without
// tokens. No screenshot is taken.
//
// Usage: node engine_menu_device_probe.mjs BASE_URL USERNAME OUTPUT
//          [ROUNDS [TRANSPORT]]
//   TRANSPORT: quic | standard (WebRTC)
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { launchChromium } from "./quality_fixture.mjs";
import { watchingMayAsk } from "./app_harness.mjs";

const [baseUrl, username, outputValue, roundsValue, transportValue] =
  process.argv.slice(2);
const board = new URL(baseUrl);
assert.equal(board.protocol, "https:");
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
const ROUNDS = Number(roundsValue ?? 5);
assert.ok(ROUNDS >= 1 && ROUNDS <= 50);
const TRANSPORT = transportValue ?? "quic";
assert.ok(["quic", "standard"].includes(TRANSPORT));
const EFFECT_LIMIT_MS = 2000;
const WAIT_MS = 6000;
const PATIENCE = 40000;
let password = "";
for await (const chunk of process.stdin) password += chunk.toString("utf8");
password = password.replace(/[\r\n]+$/, "");
assert.ok(password.length >= 8);

const now = () => performance.now();
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what, condition, patience = PATIENCE) {
  const deadline = now() + patience;
  for (;;) {
    const value = await condition();
    if (value) return value;
    assert.ok(now() < deadline, `timed out: ${what}`);
    await sleep(50);
  }
}

async function monitorRows(page) {
  for (const worker of page.workers()) {
    const found = await worker
      .evaluate(() => self.qualityMonitorRows ?? null)
      .catch(() => null);
    if (found) return found;
  }
  return null;
}

/** Frames the page drew of the picture so far, by the way it comes. */
const framesDrawn = (page) =>
  page.evaluate(() => {
    const canvas = document.querySelector(
      'canvas#screen[data-video-transport="quic"]',
    );
    if (canvas?.dataset.quicFrames !== undefined)
      return Number(canvas.dataset.quicFrames);
    const video = document.querySelector("video#screen");
    const playback = video?.getVideoPlaybackQuality?.();
    return playback ? playback.totalVideoFrames : null;
  });

const browser = await launchChromium();
const receipt = {
  schemaVersion: 1,
  probe: "h264-engine-menu-device",
  browser: browser.version(),
  rounds: ROUNDS,
  transport: TRANSPORT,
  steps: [],
  switches: [],
  hidRequests: {},
  // The page's API requests that failed (the probe's own are not the page's).
  apiErrors: [],
  errors: [],
};
const step = (name, detail = {}) => {
  receipt.steps.push({ step: name, ...detail });
  console.log(`${name}: ${JSON.stringify(detail)}`);
};

let result = "failed";
let context = null;
let csrf = "";
let found = null;
const origin = board.origin;
async function call(method, route, data) {
  const response = await context.request.fetch(`${origin}${route}`, {
    method,
    data,
    headers: { Origin: origin, "X-CSRF-Token": csrf },
    timeout: 30000,
  });
  return {
    status: response.status(),
    body: await response.json().catch(() => null),
  };
}
try {
  context = await browser.newContext({
    ignoreHTTPSErrors: true,
    locale: "en-US",
    viewport: { width: 1440, height: 1000 },
  });
  await context.addInitScript((transport) => {
    if (location.protocol !== "https:") return;
    localStorage.setItem("hardened-blikvm-transport", transport);
    localStorage.setItem("hardened-blikvm-video-mode", "h264");
    localStorage.setItem("hardened-blikvm-menu-auto-hide", "always");
    localStorage.setItem(
      "hardened-blikvm-quality-monitor",
      JSON.stringify({
        open: true,
        level: "basic",
        x: 12,
        y: 56,
        width: 340,
        height: 460,
        opaque: false,
      }),
    );
  }, TRANSPORT);
  const login = await context.request.post(`${origin}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: origin },
  });
  password = "";
  csrf = (await login.json()).data?.csrfToken;
  assert.ok(csrf, "login failed");
  const screen = (await call("GET", "/api/vm/screen")).body?.data ?? {};
  found = screen.h264Engine ?? null;
  step("found", {
    h264Engine: found,
    h264Encoder: screen.h264Encoder,
    available: screen.h264HardwareAvailable,
  });
  assert.ok(
    found === "hardware" || found === "software",
    "the board offers no engine choice",
  );

  const page = await context.newPage();
  page.setDefaultTimeout(PATIENCE);
  page.on("pageerror", (error) =>
    receipt.errors.push(String(error.message).slice(0, 200)),
  );
  page.on("request", (request) => {
    const route = new URL(request.url()).pathname;
    if (
      route.startsWith("/api/hid") &&
      !watchingMayAsk(request.method(), route)
    ) {
      const key = `${request.method()} ${route}`;
      receipt.hidRequests[key] = (receipt.hidRequests[key] ?? 0) + 1;
    }
  });
  page.on("response", (response) => {
    const route = new URL(response.url()).pathname;
    if (!route.startsWith("/api/") || route.startsWith("/api/stream/")) return;
    if (response.status() >= 400)
      receipt.apiErrors.push(`${response.status()} ${route}`);
  });
  await page.goto(`${origin}/`, { waitUntil: "domcontentloaded" });
  await page.getByTestId("menu-bar").waitFor({ timeout: 30000 });
  await until("the picture", async () => (await framesDrawn(page)) >= 2);
  step("picture", { frames: await framesDrawn(page) });

  for (let round = 0; round < ROUNDS; round += 1) {
    for (const engine of ["software", "hardware"]) {
      const encoder = engine === "software" ? "x264" : "hardware";
      const label = engine === "software" ? "H.264 SW" : "H.264 HW";
      const framesBefore = await framesDrawn(page);
      await page.locator("svg.lucide-monitor").first().click();
      await page.getByText("Video Mode", { exact: true }).hover();
      const entry = page.locator(
        `[data-video-mode="h264"][data-h264-engine="${engine}"]`,
      );
      await entry.waitFor();
      const clicked = now();
      await entry.click();
      let encoded = null;
      let shown = null;
      while (
        now() - clicked < WAIT_MS &&
        (encoded === null || shown === null)
      ) {
        if (encoded === null) {
          const said = (await call("GET", "/api/vm/screen")).body?.data
            ?.h264Encoder;
          if (said === encoder) encoded = now() - clicked;
        }
        if (shown === null) {
          const rows = await monitorRows(page);
          if (rows?.codec?.startsWith(label)) shown = now() - clicked;
        }
        await sleep(50);
      }
      // The menu closes; the picture went on.
      await page.keyboard.press("Escape");
      await page.mouse.click(1300, 900);
      await sleep(1500);
      const framesAfter = await framesDrawn(page);
      receipt.switches.push({
        engine,
        encoderMs: encoded === null ? null : Math.round(encoded),
        monitorMs: shown === null ? null : Math.round(shown),
        framesGoOn: framesAfter > framesBefore,
      });
      step(`chose ${engine}`, receipt.switches.at(-1));
      await sleep(2000);
    }
  }
  const missed = receipt.switches.filter(
    (row) =>
      row.encoderMs === null ||
      row.encoderMs > EFFECT_LIMIT_MS ||
      row.monitorMs === null ||
      !row.framesGoOn,
  );
  receipt.summary = {
    toX264Ms: receipt.switches
      .filter((row) => row.engine === "software")
      .map((row) => row.encoderMs),
    toEngineMs: receipt.switches
      .filter((row) => row.engine === "hardware")
      .map((row) => row.encoderMs),
    monitorMs: receipt.switches.map((row) => row.monitorMs),
    missed: missed.length,
  };
  assert.deepEqual(receipt.hidRequests, {}, "a HID request was made");
  assert.deepEqual(receipt.apiErrors, [], "an API request failed");
  await page.goto("about:blank");
  result =
    missed.length === 0 && receipt.errors.length === 0 ? "passed" : "failed";
} catch (error) {
  receipt.error = String(error?.message ?? error).slice(0, 400);
  console.error(receipt.error);
} finally {
  if (context && csrf) {
    try {
      // The board's choice as it was found.
      if (found) {
        const now_ = (await call("GET", "/api/vm/screen")).body?.data
          ?.h264Engine;
        if (now_ !== found) {
          await call("POST", "/api/vm/screen", {
            type: "h264_engine",
            value: found === "hardware" ? 1 : 0,
          });
        }
        receipt.left = (
          await call("GET", "/api/vm/screen")
        ).body?.data?.h264Engine;
        if (receipt.left !== found) result = "failed";
      }
      await call("POST", "/api/auth/logout", {});
    } catch (error) {
      receipt.cleanupError = String(error?.message ?? error).slice(0, 200);
      result = "failed";
    }
  }
  receipt.result = result;
  fs.writeFileSync(output, `${JSON.stringify(receipt, null, 1)}\n`);
  await browser.close();
}
console.log(`result: ${result}`);
process.exit(result === "passed" ? 0 : 1);
