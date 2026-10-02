// On a board: the H.264 picture over the QUIC session, decoded by headless
// Chromium for SECONDS at the source's size, as the page itself shows it
// (QUIC chosen, H.264 the Screen menu's mode). Measured over that span,
// after a warm-up:
// - frames decoded, and frames a second by the board's clock (the capture
//   times of the first and the last frame);
// - the age of a frame when it is decoded: from its capture to its drawing,
//   the board's time at the drawing read between two answered pings; median
//   and 95th percentile over every frame;
// - the page's own delay: from a frame's arrival to its drawing;
// - frames given up, key-frame requests, decoder restarts, and what the
//   gateway counts for the session.
//
// With LOSS and DELAY_MS the page is loaded through relays on this host
// (tools/net): a TCP relay for the page and its API, and a UDP relay for the
// session, which loses LOSS of the datagrams each way and holds each back
// for DELAY_MS. The page then has this host's address as its origin, and
// the gateway's port in the server's answers is replaced by the relay's.
//
// The probe logs in once and watches. It asks for no lease and no
// confirmation of the target: any /api/hid request fails it. The board's
// QUIC switch is turned on through the API if it is off, and is left as it
// was found, whatever happens. The Screen menu's mode and the browser's
// choice are set in this browser's own storage. The GUI password is read
// from stdin; a receipt is written under evidence/private, without tokens.
// No screenshot is taken.
//
// Usage: node quic_video_device_probe.mjs BASE_URL USERNAME OUTPUT
//          [SECONDS [LOSS DELAY_MS]]
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

import { startTcpProxy } from "../net/tcp_impair_proxy.mjs";
import { startUdpProxy } from "../net/udp_impair_proxy.mjs";
import { freePort } from "./quic_stack_rig.mjs";

const [baseUrl, username, outputValue, secondsValue, lossValue, delayValue] =
  process.argv.slice(2);
const board = new URL(baseUrl);
assert.equal(board.protocol, "https:");
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
const SECONDS = Number(secondsValue ?? 60);
assert.ok(SECONDS >= 5 && SECONDS <= 600);
const relayed = lossValue !== undefined;
const LOSS = Number(lossValue ?? 0);
const DELAY_MS = Number(delayValue ?? 0);
assert.ok(LOSS >= 0 && LOSS <= 0.2 && DELAY_MS >= 0 && DELAY_MS <= 500);
// The pings must have enclosed frames, on the path as it is, before the
// span begins.
const WARM_UP_MS = 10000;
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
const now = () => performance.now();
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(what, condition, patience = PATIENCE) {
  const deadline = now() + patience;
  for (;;) {
    const value = await condition();
    if (value) return value;
    assert.ok(now() < deadline, `timed out: ${what}`);
    await sleep(100);
  }
}

function hostClocks() {
  try {
    const [monotonic, raw] = execFileSync(
      "python3",
      [
        "-c",
        "import time;print(time.monotonic(), time.clock_gettime(time.CLOCK_MONOTONIC_RAW))",
      ],
      { encoding: "utf8" },
    )
      .trim()
      .split(" ")
      .map(Number);
    return { monotonic, raw };
  } catch {
    return null;
  }
}

const sum = (counts) => counts.reduce((total, count) => total + count, 0);
const round = (value, digits = 1) =>
  value === null ? null : Math.round(value * 10 ** digits) / 10 ** digits;

/**
 * Median, 95th and 99th percentile and the largest value of what was counted
 * between two readings of a histogram; `unit` is what one bucket stands for.
 * The last bucket holds everything beyond the range.
 */
function spread(before, after, unit) {
  const counts = after.map((count, index) => count - before[index]);
  const total = sum(counts);
  if (total === 0) return { count: 0 };
  const at = (share) => {
    const wanted = Math.max(1, Math.ceil(share * total));
    let seen = 0;
    for (let index = 0; index < counts.length; index += 1) {
      seen += counts[index];
      if (seen >= wanted) return round(index * unit);
    }
    return null;
  };
  let largest = counts.length - 1;
  while (largest > 0 && counts[largest] === 0) largest -= 1;
  return {
    count: total,
    median: at(0.5),
    p95: at(0.95),
    p99: at(0.99),
    max: round(largest * unit),
    beyondRange: counts[counts.length - 1],
    // Every value that occurred, with how often: "value": count.
    values: Object.fromEntries(
      counts
        .map((count, index) => [String(round(index * unit)), count])
        .filter(([, count]) => count > 0),
    ),
  };
}

/** The counters the canvas carries; `null` while H.264 is not on QUIC. */
const stats = (page) =>
  page.evaluate(() => {
    const canvas = document.querySelector(
      'canvas#screen[data-video-transport="quic"]',
    );
    if (!canvas || canvas.dataset.quicFrames === undefined) return null;
    return {
      frames: Number(canvas.dataset.quicFrames),
      fps: Number(canvas.dataset.quicFps),
      dropped: Number(canvas.dataset.quicDropped),
      requests: Number(canvas.dataset.quicRequests),
      resets: Number(canvas.dataset.quicResets),
    };
  });

/** What the picture's worker counts for the probes (`video.worker.ts`). */
async function videoProbe(page) {
  for (const worker of page.workers()) {
    const found = await worker
      .evaluate(() =>
        self.quicVideoProbe
          ? {
              frames: self.quicVideoProbe.frames,
              ages: Array.from(self.quicVideoProbe.ages),
              waits: Array.from(self.quicVideoProbe.waits),
              lastCaptureMicros: self.quicVideoProbe.lastCaptureMicros,
            }
          : null,
      )
      .catch(() => null);
    if (found) return found;
  }
  return null;
}

const receipt = {
  schemaVersion: 1,
  probe: "quic-video-device",
  browser: browser.version(),
  seconds: SECONDS,
  warmUpSeconds: WARM_UP_MS / 1000,
  path: relayed ? { relay: true, loss: LOSS, delayMs: DELAY_MS } : "direct",
  steps: [],
  hidRequests: {},
  errors: [],
};
const step = (name, detail = {}) => {
  receipt.steps.push({ step: name, ...detail });
  console.log(`${name}: ${JSON.stringify(detail)}`);
};

let result = "failed";
let context = null;
let csrf = "";
let switchedOn = false;
let tcp = null;
let udp = null;
// What the page is loaded from: the board, or the relays on this host.
let origin = board.origin;
const clocksBefore = hostClocks();

// A request of the login, with its CSRF token.
async function call(method, route, data) {
  const response = await context.request.fetch(`${origin}${route}`, {
    method,
    data,
    headers: { Origin: origin, "X-CSRF-Token": csrf },
    timeout: 60000,
  });
  const body = await response.json().catch(() => null);
  return { status: response.status(), body };
}

try {
  let relayPort = null;
  if (relayed) {
    relayPort = await freePort();
    tcp = await startTcpProxy({
      listen: { host: "127.0.0.1", port: relayPort },
      target: { host: board.hostname, port: Number(board.port || 443) },
    });
    origin = `https://127.0.0.1:${relayPort}`;
  }
  context = await browser.newContext({
    ignoreHTTPSErrors: true,
    locale: "en-US",
    viewport: { width: 1440, height: 1000 },
  });
  await context.addInitScript(() => {
    localStorage.setItem("hardened-blikvm-transport", "quic");
    localStorage.setItem("hardened-blikvm-video-mode", "h264");
    localStorage.setItem("hardened-blikvm-menu-auto-hide", "always");
  });
  // The path to the board answers before the one login is made over it.
  const reachable = await context.request.get(`${origin}/api/auth/setup`, {
    timeout: 20000,
  });
  assert.equal(reachable.status(), 200, "the board does not answer");
  const login = await context.request.post(`${origin}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: origin },
  });
  password = "";
  csrf = (await login.json()).data?.csrfToken;
  assert.ok(csrf, "login failed");

  const found = (await call("GET", "/api/transport")).body.data.quic;
  receipt.screen = (await call("GET", "/api/vm/screen")).body?.data ?? null;
  step("found", { quic: found, screen: receipt.screen });
  assert.ok(found.supported, "this board has no QUIC transport");
  if (!found.enabled) {
    switchedOn = true;
    const on = await call("POST", "/api/transport/quic/enable");
    assert.equal(on.status, 200);
  }
  const quic = await until("the gateway reaches the server", async () => {
    const state = (await call("GET", "/api/transport")).body.data.quic;
    return state.connected && state.port ? state : null;
  });
  if (relayed) {
    udp = await startUdpProxy({
      listen: { host: "127.0.0.1", port: relayPort },
      target: { host: board.hostname, port: quic.port },
      loss: LOSS,
      delayMs: DELAY_MS,
      seed: 20261002,
    });
    // The page may connect to its own origin only: there the relay listens.
    await context.route("**/api/transport**", async (route) => {
      const response = await route.fetch();
      const body = await response.json().catch(() => null);
      if (body?.data?.quic?.port !== undefined) body.data.quic.port = relayPort;
      if (body?.data?.port !== undefined) body.data.port = relayPort;
      await route.fulfill({ response, json: body });
    });
  }

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
  await page.goto(`${origin}/`, { waitUntil: "domcontentloaded" });
  await page.getByTestId("menu-bar").waitFor({ timeout: 30000 });

  const opened = now();
  await until("the picture comes over the session", async () => {
    const seen = await stats(page);
    // A still screen gives a frame every two seconds.
    if (seen && seen.frames >= 2) return true;
    // The page gave the picture to WebRTC: it says why in Settings.
    const fallen = await page.locator("video#screen").count();
    const notice = await page
      .locator('[data-testid="transport-notice"]')
      .getAttribute("data-reason", { timeout: 200 })
      .catch(() => null);
    assert.ok(
      !(fallen && notice),
      `the page fell back to the standard transport: ${notice}`,
    );
    return null;
  });
  step("picture", { afterMs: Math.round(now() - opened) });

  await sleep(WARM_UP_MS);
  const before = {
    probe: await videoProbe(page),
    stats: await stats(page),
    clocks: hostClocks(),
    relay: udp?.counters() ?? null,
  };
  assert.ok(before.probe, "the picture's worker was not found");
  const perSecond = [];
  for (let second = 0; second < SECONDS; second += 1) {
    await sleep(1000);
    const seen = await stats(page);
    assert.ok(seen, "the picture left the session");
    perSecond.push(seen.fps);
  }
  const after = {
    probe: await videoProbe(page),
    stats: await stats(page),
    clocks: hostClocks(),
    relay: udp?.counters() ?? null,
  };

  // The span by the board's own clock: the capture times of the newest
  // frames at its ends.
  const boardSeconds =
    (after.probe.lastCaptureMicros - before.probe.lastCaptureMicros) / 1e6;
  const frames = after.probe.frames - before.probe.frames;
  const sorted = [...perSecond].sort((left, right) => left - right);
  receipt.picture = {
    frames,
    boardSeconds: round(boardSeconds, 2),
    framesPerSecond: round(frames / boardSeconds, 2),
    // By the page's count for each second of its own clock.
    perSecond: {
      lowest: sorted[0],
      median: sorted[Math.floor(sorted.length / 2)],
      highest: sorted[sorted.length - 1],
    },
    // Capture to drawing, milliseconds.
    age: spread(before.probe.ages, after.probe.ages, 1),
    // Arrival to drawing, milliseconds.
    pageDelay: spread(before.probe.waits, after.probe.waits, 0.1),
    givenUp: after.stats.dropped - before.stats.dropped,
    keyFrameRequests: after.stats.requests - before.stats.requests,
    decoderRestarts: after.stats.resets - before.stats.resets,
  };
  if (before.clocks && after.clocks) {
    receipt.hostSeconds = {
      monotonic: round(after.clocks.monotonic - before.clocks.monotonic, 2),
      raw: round(after.clocks.raw - before.clocks.raw, 2),
    };
  }
  if (after.relay) {
    const passed = (direction) => ({
      received:
        after.relay[direction].received - before.relay[direction].received,
      dropped: after.relay[direction].dropped - before.relay[direction].dropped,
    });
    receipt.relay = {
      toBoard: passed("toTarget"),
      toPage: passed("toClient"),
    };
  }
  step("measured", receipt.picture);

  // What the page and the gateway say about the session.
  await page.locator("svg.lucide-settings").first().click();
  await page.locator('[data-settings-tab="system"]').click();
  const status = page.getByTestId("transport-status");
  await status.waitFor();
  const line = page.getByTestId("transport-video");
  receipt.session = {
    state: await status.getAttribute("data-state"),
    says: await status.innerText(),
    gateway: {
      framesSent: Number(await status.getAttribute("data-frames-sent")),
      framesDropped: Number(await status.getAttribute("data-frames-dropped")),
      keyFrames: Number(await status.getAttribute("data-key-frames")),
      packetsLost: Number(await status.getAttribute("data-packets-lost")),
    },
    video: await line.getAttribute("data-video"),
    videoSays: await line.innerText(),
  };
  step("session", receipt.session);
  assert.equal(receipt.session.state, "connected");
  assert.equal(receipt.session.video, "quic");
  assert.ok(frames > 0, "no frame was decoded");
  assert.deepEqual(receipt.hidRequests, {}, "a HID request was made");
  assert.deepEqual(receipt.errors, []);

  // The page leaves, as a closed tab does.
  await page.goto("about:blank");
  result = "passed";
} catch (error) {
  receipt.error = String(error?.message ?? error).slice(0, 400);
  console.error(receipt.error);
} finally {
  // The switch as it was found, and the login ended.
  if (context && csrf) {
    try {
      if (switchedOn) {
        const off = await call("POST", "/api/transport/quic/disable");
        receipt.switchedOff = off.status === 200;
        const state = (await call("GET", "/api/transport")).body?.data?.quic;
        receipt.left = { enabled: state?.enabled, connected: state?.connected };
      }
      await call("POST", "/api/auth/logout", {});
    } catch (error) {
      receipt.cleanupError = String(error?.message ?? error).slice(0, 200);
      result = "failed";
    }
  }
  if (switchedOn && receipt.left?.enabled !== false) result = "failed";
  const clocksAfter = hostClocks();
  if (clocksBefore && clocksAfter) {
    receipt.hostClockRate =
      Math.round(
        ((clocksAfter.monotonic - clocksBefore.monotonic) /
          (clocksAfter.raw - clocksBefore.raw)) *
          10000,
      ) / 10000;
  }
  receipt.result = result;
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1));
  await browser.close();
  await udp?.close();
  await tcp?.close();
}
console.log(`result: ${result}`);
process.exit(result === "passed" ? 0 : 1);
