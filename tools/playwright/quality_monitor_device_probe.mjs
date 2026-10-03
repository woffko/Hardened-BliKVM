// On a board: the quality monitor with the picture over WebRTC (H.264 or
// VP8) or MJPEG, as the page itself shows it, against what this probe
// counts (docs/design/h264-engine-choice-and-quality-monitor-v1.md, C5.2).
// The page's own WebRTC connection is kept by the probe when the page makes
// it, and read with getStats() at the span's ends: the frames the video
// element drew a second and the bytes that came, beside what the monitor
// showed every second of the span. Over MJPEG the monitor shows the board's
// capture only, and so does this receipt. QUIC has its own probe
// (quic_video_device_probe.mjs with QUALITY_MONITOR).
//
// The probe logs in once and watches. It asks for no lease and no
// confirmation of the target: any /api/hid request fails it. The mode is set
// in this browser's own storage. The GUI password is read from stdin; a
// receipt is written under evidence/private, without tokens. No screenshot
// is taken.
//
// Usage: node quality_monitor_device_probe.mjs BASE_URL USERNAME OUTPUT MODE
//          [SECONDS [LEVEL]]
//   MODE: h264 | vp8 | mjpeg (over the standard transport)
//   LEVEL: basic | extended
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { launchChromium } from "./quality_fixture.mjs";

const [baseUrl, username, outputValue, mode, secondsValue, levelValue] =
  process.argv.slice(2);
const board = new URL(baseUrl);
assert.equal(board.protocol, "https:");
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
assert.ok(["h264", "vp8", "mjpeg"].includes(mode));
const SECONDS = Number(secondsValue ?? 30);
assert.ok(SECONDS >= 5 && SECONDS <= 600);
const LEVEL = levelValue ?? "basic";
assert.ok(["basic", "extended"].includes(LEVEL));
const WARM_UP_MS = 8000;
const PATIENCE = 40000;
let password = "";
for await (const chunk of process.stdin) password += chunk.toString("utf8");
password = password.replace(/[\r\n]+$/, "");
assert.ok(password.length >= 8);

const now = () => performance.now();
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const round = (value, digits = 2) =>
  value === null || value === undefined || Number.isNaN(value)
    ? null
    : Math.round(value * 10 ** digits) / 10 ** digits;
const mean = (values) =>
  values.length
    ? values.reduce((sum, value) => sum + value, 0) / values.length
    : null;
async function until(what, condition, patience = PATIENCE) {
  const deadline = now() + patience;
  for (;;) {
    const value = await condition();
    if (value) return value;
    assert.ok(now() < deadline, `timed out: ${what}`);
    await sleep(100);
  }
}

/** What the monitor's worker drew last, by row, and how often it drew. */
async function monitorDraw(page) {
  for (const worker of page.workers()) {
    const found = await worker
      .evaluate(() =>
        self.qualityMonitorRows
          ? { rows: self.qualityMonitorRows, draws: self.qualityMonitorDraws }
          : null,
      )
      .catch(() => null);
    if (found) return found;
  }
  return null;
}

/** The page's WebRTC picture as its own connection and video element say. */
const pictureCounts = (page) =>
  page.evaluate(async () => {
    const peer = (window.__probePeers ?? []).at(-1);
    const video = document.querySelector("video#screen");
    const playback = video?.getVideoPlaybackQuality?.();
    let inbound = null;
    if (peer) {
      (await peer.getStats()).forEach((stat) => {
        if (stat.type === "inbound-rtp" && stat.kind === "video")
          inbound = stat;
      });
    }
    return {
      at: performance.now(),
      drawn: playback
        ? playback.totalVideoFrames - playback.droppedVideoFrames
        : null,
      decoded: inbound?.framesDecoded ?? null,
      bytes: inbound?.bytesReceived ?? null,
      codec: inbound?.codecId ?? null,
    };
  });

const browser = await launchChromium();
const receipt = {
  schemaVersion: 1,
  probe: "quality-monitor-device",
  browser: browser.version(),
  mode,
  level: LEVEL,
  seconds: SECONDS,
  steps: [],
  hidRequests: {},
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
const origin = board.origin;
try {
  context = await browser.newContext({
    ignoreHTTPSErrors: true,
    locale: "en-US",
    viewport: { width: 1440, height: 1000 },
  });
  await context.addInitScript(
    ([videoMode, level]) => {
      // Not on the blank page the probe leaves for.
      if (location.protocol !== "https:") return;
      localStorage.setItem("hardened-blikvm-transport", "standard");
      localStorage.setItem("hardened-blikvm-video-mode", videoMode);
      localStorage.setItem("hardened-blikvm-menu-auto-hide", "always");
      localStorage.setItem(
        "hardened-blikvm-quality-monitor",
        JSON.stringify({
          open: true,
          level,
          x: 12,
          y: 56,
          width: 380,
          height: 640,
          opaque: false,
        }),
      );
      // The page's own connection, kept for the probe's getStats().
      const Original = window.RTCPeerConnection;
      if (Original) {
        window.__probePeers = [];
        const Kept = function (...args) {
          const peer = new Original(...args);
          window.__probePeers.push(peer);
          return peer;
        };
        Kept.prototype = Original.prototype;
        window.RTCPeerConnection = Kept;
      }
    },
    [mode, LEVEL],
  );
  const login = await context.request.post(`${origin}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: origin },
  });
  password = "";
  csrf = (await login.json()).data?.csrfToken;
  assert.ok(csrf, "login failed");

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
  page.on("response", async (response) => {
    const route = new URL(response.url()).pathname;
    if (!route.startsWith("/api/") || route.startsWith("/api/stream/")) return;
    if (response.status() >= 400)
      receipt.apiErrors.push(`${response.status()} ${route}`);
  });
  await page.goto(`${origin}/`, { waitUntil: "domcontentloaded" });
  await page.getByTestId("menu-bar").waitFor({ timeout: 30000 });
  await page.getByTestId("quality-monitor").waitFor({ timeout: 30000 });

  if (mode !== "mjpeg") {
    const began = now();
    await until("the picture comes over WebRTC", async () => {
      const counts = await pictureCounts(page);
      return counts.decoded > 0 ? counts : null;
    });
    step("picture", { afterMs: Math.round(now() - began) });
  }
  await sleep(WARM_UP_MS);
  const before = mode === "mjpeg" ? null : await pictureCounts(page);
  // Every draw of the monitor, read ten times a second so that none is
  // missed: its per-second values move in steps, and reading once a second
  // could meet the same step every time.
  const shown = [];
  let lastDraw = null;
  let missedDraws = 0;
  const spanStart = now();
  while (now() - spanStart < SECONDS * 1000) {
    const seen = await monitorDraw(page);
    if (seen && seen.draws !== lastDraw) {
      if (lastDraw !== null && seen.draws > lastDraw + 1)
        missedDraws += seen.draws - lastDraw - 1;
      lastDraw = seen.draws;
      shown.push(seen.rows);
    }
    await sleep(100);
  }
  const after = mode === "mjpeg" ? null : await pictureCounts(page);
  assert.ok(shown.length >= SECONDS / 2, "the monitor drew too seldom");
  const last = shown.at(-1);
  const number = (text) => Number.parseFloat(text);
  receipt.monitor = {
    draws: shown.length,
    missedDraws,
    path: last.path,
    codec: last.codec,
    bitrate: last.bitrate,
    chroma: last.chroma,
    fps: last.fps,
    speedMbitMean: round(
      mean(shown.map((rows) => number(rows.speed)).filter(Number.isFinite)),
      3,
    ),
    drawnMean: round(
      mean(shown.map((rows) => number(rows.fps)).filter(Number.isFinite)),
    ),
    delays: [...new Set(shown.map((rows) => rows.delay))].slice(0, 12),
    // Every draw's Speed as it was shown.
    speeds: shown.map((rows) => rows.speed),
    rows: last,
  };
  if (before && after) {
    const seconds = (after.at - before.at) / 1000;
    receipt.counted = {
      drawnPerSecond: round((after.drawn - before.drawn) / seconds),
      decodedPerSecond: round((after.decoded - before.decoded) / seconds),
      megabitsPerSecond: round(
        ((after.bytes - before.bytes) * 8) / 1e6 / seconds,
        3,
      ),
    };
    const within = (shownValue, counted) =>
      shownValue !== null && Math.abs(shownValue - counted) <= 0.1 * counted;
    receipt.checks = {
      framesPerSecondWithin10: within(
        receipt.monitor.drawnMean,
        receipt.counted.drawnPerSecond,
      ),
      bytesRateWithin10: within(
        receipt.monitor.speedMbitMean,
        receipt.counted.megabitsPerSecond,
      ),
      path: receipt.monitor.path === "WebRTC",
    };
  } else {
    receipt.checks = { path: receipt.monitor.path === "MJPEG" };
  }
  step("measured", {
    monitor: receipt.monitor,
    counted: receipt.counted ?? null,
  });
  assert.deepEqual(receipt.hidRequests, {}, "a HID request was made");
  assert.deepEqual(receipt.apiErrors, [], "an API request failed");
  await page.goto("about:blank");
  result = Object.values(receipt.checks).every(Boolean) ? "passed" : "failed";
} catch (error) {
  receipt.error = String(error?.message ?? error).slice(0, 400);
  console.error(receipt.error);
} finally {
  if (context && csrf) {
    try {
      await context.request.post(`${origin}/api/auth/logout`, {
        data: {},
        headers: { Origin: origin, "X-CSRF-Token": csrf },
      });
    } catch (error) {
      receipt.cleanupError = String(error?.message ?? error).slice(0, 200);
    }
  }
  receipt.result = result;
  fs.writeFileSync(output, `${JSON.stringify(receipt, null, 1)}\n`);
  await browser.close();
}
console.log(`result: ${result}`);
process.exit(result === "passed" ? 0 : 1);
