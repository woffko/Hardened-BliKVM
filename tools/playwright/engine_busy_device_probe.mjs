// On a board: the video engine busy while this page watches H.264
// (docs/baselines/work-plan-20261004-review-fixes.md, F3 e). Another process
// holds the engine's device node (`tools/device/ve_device_hold.py`, started by
// the one who runs this probe, with nobody watching); this page comes in H.264
// and the probe checks that
//   - x264 encodes while the engine is held, and the encoder's report, the
//     quality monitor and the video menu say why (the engine is busy);
//   - the menu's H.264 HW entry stays choosable and says that choosing it asks
//     the encoder to try again;
//   - once the holder has let go (its log says so), one click on H.264 HW
//     brings the engine back within two seconds, the monitor shows it, and the
//     picture goes on.
// At the end the board's choice is the one the probe found, whatever happens.
//
// The probe logs in once and watches. It asks for no lease and no
// confirmation of the target: any /api/hid request fails it. The transport
// and the mode are set in this browser's own storage. The GUI password is
// read from stdin; a receipt is written under evidence/private, without
// tokens. No screenshot is taken.
//
// Usage: node engine_busy_device_probe.mjs BASE_URL USERNAME OUTPUT HOLDER_LOG
//          [TRANSPORT]
//   HOLDER_LOG: the file the holder's output goes to ("holding ..." when it
//     has the device, "released ..." when it has let go)
//   TRANSPORT: quic | standard (WebRTC)
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { launchChromium } from "./quality_fixture.mjs";

const [baseUrl, username, outputValue, holderLogValue, transportValue] =
  process.argv.slice(2);
const board = new URL(baseUrl);
assert.equal(board.protocol, "https:");
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
const holderLog = path.resolve(holderLogValue);
const TRANSPORT = transportValue ?? "quic";
assert.ok(["quic", "standard"].includes(TRANSPORT));
const EFFECT_LIMIT_MS = 2000;
const PATIENCE = 40000;
const HOLDER_PATIENCE = 120000;
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
const holderSays = (word) => {
  try {
    return fs
      .readFileSync(holderLog, "utf8")
      .split("\n")
      .some((line) => line.startsWith(word));
  } catch {
    return false;
  }
};

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
  probe: "h264-engine-busy-device",
  browser: browser.version(),
  transport: TRANSPORT,
  steps: [],
  held: {},
  released: {},
  hidRequests: {},
  engineChoices: [],
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
const screenState = async () =>
  (await call("GET", "/api/vm/screen")).body?.data ?? {};
const entry = (page, engine) =>
  page.locator(`[data-video-mode="h264"][data-h264-engine="${engine}"]`);
async function openMenu(page) {
  await page.locator("svg.lucide-monitor").first().click();
  await page.getByText("Video Mode", { exact: true }).hover();
  await entry(page, "hardware").waitFor();
}
async function closeMenu(page) {
  await page.keyboard.press("Escape");
  await page.mouse.click(1300, 900);
  await sleep(500);
}

try {
  assert.ok(holderSays("holding"), "the holder does not hold the engine");
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
  const screen = await screenState();
  found = screen.h264Engine ?? null;
  step("found", {
    h264Engine: found,
    h264Encoder: screen.h264Encoder,
    available: screen.h264HardwareAvailable,
  });
  assert.equal(found, "hardware", "the board's choice is not the engine");

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
    if (request.method() === "POST" && route === "/api/vm/screen") {
      const body = request.postDataJSON?.() ?? null;
      if (body?.type === "h264_engine") receipt.engineChoices.push(body.value);
    }
  });
  page.on("response", (response) => {
    const route = new URL(response.url()).pathname;
    if (!route.startsWith("/api/") || route.startsWith("/api/stream/")) return;
    if (response.status() >= 400)
      receipt.apiErrors.push(`${response.status()} ${route}`);
  });
  const arrived = now();
  await page.goto(`${origin}/`, { waitUntil: "domcontentloaded" });
  await page.getByTestId("menu-bar").waitFor({ timeout: 30000 });
  await until("the picture", async () => (await framesDrawn(page)) >= 2);
  step("picture", { frames: await framesDrawn(page) });

  // The engine is held: x264 encodes, and everything says why.
  const held = await until("x264 while the engine is held", async () => {
    const state = await screenState();
    return state.h264Encoder === "x264" &&
      state.h264HardwareAvailable === false &&
      /busy/i.test(state.h264HardwareReason ?? "")
      ? state
      : null;
  });
  receipt.held.screen = {
    h264Engine: held.h264Engine,
    h264Encoder: held.h264Encoder,
    available: held.h264HardwareAvailable,
    reason: held.h264HardwareReason,
    afterArrivalMs: Math.round(now() - arrived),
  };
  const quality = (await call("GET", "/api/vm/quality")).body?.data ?? {};
  receipt.held.quality = {
    engine: quality.h264?.engine ?? null,
    wanted: quality.h264?.engineWanted ?? null,
    available: quality.h264?.hardwareAvailable ?? null,
    reason: quality.h264?.hardwareReason ?? null,
  };
  assert.equal(receipt.held.quality.engine, "x264");
  assert.equal(receipt.held.quality.available, false);
  assert.match(receipt.held.quality.reason ?? "", /busy/i);
  const rows = await until("the monitor's rows", () => monitorRows(page));
  receipt.held.monitorCodec = rows.codec ?? null;
  assert.ok(
    String(rows.codec).startsWith("H.264 SW"),
    `the monitor shows ${rows.codec}`,
  );
  await openMenu(page);
  const hardware = entry(page, "hardware");
  receipt.held.menu = {
    disabled: await hardware.getAttribute("aria-disabled"),
    checked: await hardware.getAttribute("data-checked"),
    note:
      (await hardware.locator("[data-h264-encodes]").count()) > 0
        ? await hardware.locator("[data-h264-encodes]").innerText()
        : null,
  };
  await hardware.hover();
  await page.getByText(/busy/i).first().waitFor();
  await page.getByText("asks the encoder to try again").waitFor();
  receipt.held.menu.tooltip = "the reason and the retry text";
  step("held", receipt.held);
  assert.equal(receipt.held.menu.disabled, "false");
  assert.equal(receipt.held.menu.note, "x264 encodes");
  await closeMenu(page);
  const framesHeld = await framesDrawn(page);
  await sleep(1500);
  receipt.held.framesGoOn = (await framesDrawn(page)) > framesHeld;
  assert.ok(receipt.held.framesGoOn, "the picture stopped while held");

  // The holder lets go; the choice made again brings the engine back.
  await until(
    "the holder letting go",
    async () => holderSays("released"),
    HOLDER_PATIENCE,
  );
  step("holder released");
  await sleep(500);
  const framesBefore = await framesDrawn(page);
  await openMenu(page);
  const clicked = now();
  await entry(page, "hardware").click();
  let encoded = null;
  let shown = null;
  while (now() - clicked < 6000 && (encoded === null || shown === null)) {
    if (encoded === null) {
      const state = await screenState();
      if (state.h264Encoder === "hardware" && state.h264HardwareAvailable)
        encoded = now() - clicked;
    }
    if (shown === null) {
      const rows = await monitorRows(page);
      if (rows?.codec?.startsWith("H.264 HW")) shown = now() - clicked;
    }
    await sleep(50);
  }
  await closeMenu(page);
  await sleep(1500);
  const after = await screenState();
  receipt.released = {
    encoderMs: encoded === null ? null : Math.round(encoded),
    monitorMs: shown === null ? null : Math.round(shown),
    h264Engine: after.h264Engine,
    h264Encoder: after.h264Encoder,
    available: after.h264HardwareAvailable,
    reason: after.h264HardwareReason ?? null,
    framesGoOn: (await framesDrawn(page)) > framesBefore,
  };
  step("clicked H.264 HW", receipt.released);
  assert.ok(
    encoded !== null && encoded <= EFFECT_LIMIT_MS,
    `the engine came back after ${encoded}`,
  );
  assert.ok(shown !== null, "the monitor did not show the engine");
  assert.ok(receipt.released.framesGoOn, "the picture stopped");
  assert.deepEqual(receipt.engineChoices, [1], "one choice of the engine");
  assert.deepEqual(receipt.hidRequests, {}, "a HID request was made");
  assert.deepEqual(receipt.apiErrors, [], "an API request failed");
  await page.goto("about:blank");
  result = receipt.errors.length === 0 ? "passed" : "failed";
} catch (error) {
  receipt.error = String(error?.message ?? error).slice(0, 400);
  console.error(receipt.error);
} finally {
  if (context && csrf) {
    try {
      // The board's choice as it was found.
      if (found) {
        const now_ = (await screenState()).h264Engine;
        if (now_ !== found) {
          await call("POST", "/api/vm/screen", {
            type: "h264_engine",
            value: found === "hardware" ? 1 : 0,
          });
        }
        receipt.left = (await screenState()).h264Engine;
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
