// The picture over the QUIC transport at the board's size and rate, on the
// development host: 1920x1080, 30 frames a second, 2.5 Mbit/s, from the
// encoder's own x264 engine behind a frame socket (h264_frame_source),
// through the real gateway and a UDP relay to headless Chromium, with a
// played server (quic_fixture.mjs). The picture moves all the time, which a
// board's target does only when somebody works on it.
//
// For each path (the relay without impairment, 1% and 3% of the datagrams
// lost each way, 50 ms added each way) a page watches for SECONDS after a
// warm-up, and what its worker counted is read: frames decoded, frames a
// second by the source's clock, the age of a frame at its decoding (the
// gateway and the page share this host's clock), the page's own delay,
// frames given up, key-frame requests, and what the gateway and the relay
// counted.
//
// Usage: node quic_video_measure.mjs OUTPUT.json [SECONDS]
//   BLI_KVM_QUIC_GATEWAY and BLI_KVM_H264_FRAME_SOURCE name the builds;
//   optimized ones are meant (target/release).
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { startUdpProxy } from "../net/udp_impair_proxy.mjs";
import { startApp } from "./app_harness.mjs";
import {
  Board,
  GATEWAY,
  launch,
  openPage,
  openSettings,
  sleep,
  startFrameSource,
  until,
  videoProbe,
} from "./quic_fixture.mjs";
import { freePort } from "./quic_stack_rig.mjs";

const [outputValue, secondsValue] = process.argv.slice(2);
assert.ok(outputValue, "usage: quic_video_measure.mjs OUTPUT.json [SECONDS]");
const output = path.resolve(outputValue);
assert.ok(!fs.existsSync(output), `${output} exists`);
const SECONDS = Number(secondsValue ?? 60);
assert.ok(SECONDS >= 5 && SECONDS <= 600);
const WARM_UP_MS = 6000;
const SOURCE = { width: 1920, height: 1080, fps: 30, kbps: 2500 };
const PATHS = [
  { name: "clean", loss: 0, delayMs: 0 },
  { name: "1% loss", loss: 0.01, delayMs: 0 },
  { name: "3% loss", loss: 0.03, delayMs: 0 },
  { name: "50 ms each way", loss: 0, delayMs: 50 },
];

const sum = (counts) => counts.reduce((total, count) => total + count, 0);
const round = (value, digits = 1) =>
  value === null ? null : Math.round(value * 10 ** digits) / 10 ** digits;
/** What was counted between two readings of a histogram, `unit` a bucket. */
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
  };
}
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

const browser = await launch();
const app = await startApp();
const PORT = Number(new URL(app.baseUrl).port);
const directory = fs.mkdtempSync(path.join(os.tmpdir(), "quic-video-"));
const frameSocket = path.join(directory, "frames.sock");
const source = await startFrameSource(frameSocket, SOURCE);
const gatewayPort = await freePort();
const relay = await startUdpProxy({
  listen: { host: "127.0.0.1", port: PORT },
  target: { host: "127.0.0.1", port: gatewayPort },
  seed: 20261002,
});
const board = new Board(PORT, { gatewayPort, frameSocket });
await board.listen();
board.enabled = true;
board.startGateway();
await board.gatewayOnTheLink();

const receipt = {
  schemaVersion: 1,
  probe: "quic-video-host",
  browser: browser.version(),
  gateway: path.relative(process.cwd(), GATEWAY),
  source: SOURCE,
  seconds: SECONDS,
  warmUpSeconds: WARM_UP_MS / 1000,
  paths: [],
};
let result = "failed";
try {
  for (const condition of PATHS) {
    relay.impair({ loss: condition.loss, delayMs: condition.delayMs });
    const { context, page, errors } = await openPage(browser, app, board, {
      choice: "quic",
      videoMode: "h264",
      prepare: (context) =>
        context.addInitScript(() =>
          localStorage.setItem("hardened-blikvm-menu-auto-hide", "always"),
        ),
    });
    await until("the picture comes over the session", async () => {
      const seen = await stats(page);
      return seen && seen.frames >= 30 ? seen : null;
    });
    await sleep(WARM_UP_MS);
    const before = {
      probe: await videoProbe(page),
      stats: await stats(page),
      relay: relay.counters(),
    };
    const perSecond = [];
    for (let second = 0; second < SECONDS; second += 1) {
      await sleep(1000);
      perSecond.push((await stats(page)).fps);
    }
    const after = {
      probe: await videoProbe(page),
      stats: await stats(page),
      relay: relay.counters(),
    };
    await openSettings(page);
    const status = page.locator('[data-testid="transport-status"]');
    const sourceSeconds =
      (after.probe.lastCaptureMicros - before.probe.lastCaptureMicros) / 1e6;
    const frames = after.probe.frames - before.probe.frames;
    const sorted = [...perSecond].sort((left, right) => left - right);
    const passed = (direction) => ({
      received:
        after.relay[direction].received - before.relay[direction].received,
      dropped: after.relay[direction].dropped - before.relay[direction].dropped,
    });
    const entry = {
      path: condition.name,
      relay: { loss: condition.loss, delayMs: condition.delayMs },
      frames,
      sourceSeconds: round(sourceSeconds, 2),
      framesPerSecond: round(frames / sourceSeconds, 2),
      perSecond: {
        lowest: sorted[0],
        median: sorted[Math.floor(sorted.length / 2)],
      },
      age: spread(before.probe.ages, after.probe.ages, 1),
      pageDelay: spread(before.probe.waits, after.probe.waits, 0.1),
      givenUp: after.stats.dropped - before.stats.dropped,
      keyFrameRequests: after.stats.requests - before.stats.requests,
      decoderRestarts: after.stats.resets - before.stats.resets,
      says: await status.innerText(),
      gatewaySinceTheSessionBegan: {
        framesSent: Number(await status.getAttribute("data-frames-sent")),
        framesDropped: Number(await status.getAttribute("data-frames-dropped")),
        keyFrames: Number(await status.getAttribute("data-key-frames")),
        packetsLost: Number(await status.getAttribute("data-packets-lost")),
      },
      datagrams: { toGateway: passed("toTarget"), toPage: passed("toClient") },
    };
    receipt.paths.push(entry);
    console.log(
      `${condition.name}: ${entry.framesPerSecond} frames a second, ` +
        `age median ${entry.age.median} p95 ${entry.age.p95} ms, ` +
        `page ${entry.pageDelay.median} ms, given up ${entry.givenUp}, ` +
        `key-frame requests ${entry.keyFrameRequests}`,
    );
    assert.deepEqual(errors, []);
    await page.goto("about:blank");
    await until("the page's sessions are closed", () =>
      board.sessions.length === 0 ? true : null,
    );
    await context.close();
  }
  result = "passed";
} catch (error) {
  receipt.error = String(error?.message ?? error).slice(0, 400);
  console.error(receipt.error);
  if (board.stderr) console.error(`gateway:\n${board.stderr}`);
} finally {
  receipt.result = result;
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1));
  await browser.close();
  await board.close();
  await relay.close();
  await source.stop();
  app.server.close();
  fs.rmSync(directory, { recursive: true, force: true });
}
console.log(`result: ${result}`);
process.exit(result === "passed" ? 0 : 1);
