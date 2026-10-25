// Gate C4 of the H.264 engine choice and quality monitor plan
// (docs/design/h264-engine-choice-and-quality-monitor-v1.md), on the host:
// the picture over QUIC through the real web server, the real gateway and
// the real encoder (x264 on the host), which a stand-in for uStreamer feeds
// with changing pictures. With the quality monitor open the script compares
// what the monitor shows with what it counts itself over the same span:
// frames drawn a second (the picture's worker, as the QUIC probes read it),
// the bytes' rate (every frame the encoder handed to the gateway, read from
// its frame socket), and a frame's age when drawn (the worker's own ages).
// It then measures what the open monitor costs the page's main thread
// (Chromium's trace of windows with the monitor closed and open in turn).
// It also shows the board's side of the monitor filled from the encoder's
// real report. The receipt is written under evidence/private.
//
// The server is the board's build, with the owner's surface
// (BLI_KVM_SERVER names it: `cargo build -p bli-kvm-server --features
// restricted-control-surface`); a stand-in for the owner says the QUIC
// switch is on and what the capture runs with.
//
// usage: node tools/playwright/quality_monitor_gate.mjs [OUTPUT]
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { launchChromium } from "./quality_fixture.mjs";
import { CHOICE_KEY, VIDEO_MODE_KEY, videoProbe } from "./quic_fixture.mjs";
import { binary, freePort } from "./quic_stack_rig.mjs";

const OUTPUT = path.resolve(
  process.argv[2] ?? `evidence/private/c4-gate-${Date.now()}.json`,
);
assert.equal(path.dirname(OUTPUT), path.resolve("evidence/private"));
const FPS = 30;
const SPAN_MS = 12000;
/** Closed and open in turn, this long each, this many times for each level. */
const COST_WINDOW_MS = 8000;
const COST_PAIRS = 7;
const PATIENCE_MS = 20000;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const now = () => performance.now();

/** A process of the stack, and what it said. */
function run(file, env, name) {
  const child = spawn(file, [], {
    env: { PATH: process.env.PATH, ...env },
    stdio: ["ignore", "ignore", "pipe"],
  });
  const state = { log: "", exited: false };
  child.stderr.on("data", (chunk) => (state.log += chunk));
  child.once("exit", () => (state.exited = true));
  state.said = async (pattern) => {
    const deadline = now() + PATIENCE_MS;
    while (!pattern.test(state.log)) {
      assert.ok(!state.exited, `${name} ended:\n${state.log}`);
      assert.ok(
        now() < deadline,
        `${name} did not say ${pattern}:\n${state.log}`,
      );
      await sleep(20);
    }
  };
  state.stop = () =>
    new Promise((resolve) => {
      if (state.exited) return resolve();
      child.once("exit", resolve);
      child.kill("SIGTERM");
    });
  return state;
}

/**
 * The board owner as the server meets it here: the QUIC switch on and
 * running, the capture at 30 frames a second and JPEG quality 80; every
 * other request applied.
 */
function startOwner(socketPath) {
  const sockets = new Set();
  const server = net.createServer({ allowHalfOpen: true }, (socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
    socket.on("error", () => undefined);
    let buffer = Buffer.alloc(0);
    socket.on("end", () => socket.end());
    socket.on("data", (chunk) => {
      buffer = Buffer.concat([buffer, chunk]);
      while (buffer.length >= 4) {
        const length = buffer.readUInt32BE(0);
        if (buffer.length < 4 + length) return;
        const text = buffer.subarray(4, 4 + length).toString("utf8");
        buffer = buffer.subarray(4 + length);
        const requestId = /"requestId":(\d+)/.exec(text)?.[1];
        const operationId = /"operationId":"([0-9a-f]+)"/.exec(text)?.[1];
        const type = /"request":\{"type":"([a-z_]+)"/.exec(text)?.[1];
        const response =
          type === "quic_state"
            ? '{"type":"quic_state","enabled":true,"active":true}'
            : type === "video_settings_state"
              ? '{"type":"video_state","fps":30,"quality":80,"resolution":"1920x1080"}'
              : '{"type":"applied"}';
        const body = Buffer.from(
          `{"schemaVersion":2,"requestId":${requestId},"operationId":"${operationId}","response":${response}}`,
        );
        const prefix = Buffer.alloc(4);
        prefix.writeUInt32BE(body.length);
        socket.write(Buffer.concat([prefix, body]));
      }
    });
  });
  return new Promise((resolve) =>
    server.listen(socketPath, () =>
      resolve({
        close: () =>
          new Promise((done) => {
            for (const socket of sockets) socket.destroy();
            server.close(done);
          }),
      }),
    ),
  );
}

/** uStreamer's side of its socket: three pictures in turn, FPS a second. */
function capture(socketPath) {
  const fixtures = path.resolve("crates/bli-kvm-video-encoder/tests/fixtures");
  const pictures = ["420", "422", "444"].map((kind) =>
    fs.readFileSync(path.join(fixtures, `pattern-320x180-${kind}.jpg`)),
  );
  const server = net.createServer((connection) => {
    let request = "";
    let timer = null;
    connection.on("error", () => clearInterval(timer));
    connection.on("close", () => clearInterval(timer));
    connection.on("data", (chunk) => {
      if (timer) return;
      request += chunk.toString("latin1");
      if (!request.includes("\r\n\r\n")) return;
      connection.write(
        "HTTP/1.0 200 OK\r\nContent-Type: multipart/x-mixed-replace;boundary=frame\r\n\r\n",
      );
      let index = 0;
      timer = setInterval(() => {
        const jpeg = pictures[index % pictures.length];
        index += 1;
        connection.write(
          `--frame\r\nContent-Type: image/jpeg\r\nContent-Length: ${jpeg.length}\r\n\r\n`,
        );
        connection.write(jpeg);
        connection.write("\r\n");
      }, 1000 / FPS);
    });
  });
  return new Promise((resolve) =>
    server.listen(socketPath, () => resolve(server)),
  );
}

/** The demand side's stand-in is the server itself; this reads the frames
 * the encoder hands to the gateway, as a second client of its socket. */
function tapFrames(socketPath) {
  const tap = { bytes: 0, frames: 0, connection: null };
  const connection = net.connect(socketPath);
  let pending = Buffer.alloc(0);
  connection.on("data", (chunk) => {
    pending = Buffer.concat([pending, chunk]);
    for (;;) {
      if (pending.length < 32) break;
      const size = pending.readUInt32BE(20);
      if (pending.length < 32 + size) break;
      tap.bytes += size;
      tap.frames += 1;
      pending = pending.subarray(32 + size);
    }
  });
  connection.on("error", () => undefined);
  tap.connection = connection;
  return tap;
}

/** What the monitor's worker drew last: every row's value by its key, the
 * labels, and the sections. */
async function drawn(page) {
  for (const worker of page.workers()) {
    const found = await worker
      .evaluate(() =>
        self.qualityMonitorRows
          ? {
              rows: self.qualityMonitorRows,
              labels: self.qualityMonitorLabels,
              sections: self.qualityMonitorSections,
            }
          : null,
      )
      .catch(() => null);
    if (found) return found;
  }
  return { rows: {}, labels: {}, sections: [] };
}
const rows = async (page) => (await drawn(page)).rows;

/** The median of the ages counted between two reads of the worker's probe. */
function medianAge(before, after) {
  const counts = after.ages.map(
    (count, index) => count - (before.ages[index] ?? 0),
  );
  const total = counts.reduce((sum, count) => sum + count, 0);
  let seen = 0;
  for (const [age, count] of counts.entries()) {
    seen += count;
    if (seen * 2 >= total) return age;
  }
  return null;
}

const mean = (values) =>
  values.length
    ? values.reduce((sum, value) => sum + value, 0) / values.length
    : null;
const round = (value, places = 2) =>
  value === null ? null : Math.round(value * 10 ** places) / 10 ** places;

const directory = fs.mkdtempSync(path.join(os.tmpdir(), "quality-gate-"));
const files = {
  control: path.join(directory, "control.sock"),
  link: path.join(directory, "link.sock"),
  ustreamer: path.join(directory, "ustreamer.sock"),
  demand: path.join(directory, "demand.sock"),
  frames: path.join(directory, "frames.sock"),
  report: path.join(directory, "report.json"),
  status: path.join(directory, "encoder"),
};
const receipt = { gate: "C4", fps: FPS, spanMs: SPAN_MS };
const processes = [];
let browser = null;
let owner = null;
let camera = null;
let tap = null;
let result = "failed";
try {
  owner = await startOwner(files.control);
  camera = await capture(files.ustreamer);
  const rtp = await freePort();
  const encoder = run(
    binary("BLI_KVM_VIDEO_ENCODER", "bli-kvm-video-encoder"),
    {
      BLI_KVM_USTREAMER_SOCKET: files.ustreamer,
      BLI_KVM_ENCODER_CODEC: "h264-ultrafast",
      BLI_KVM_ENCODER_RTP_PORT: String(rtp),
      BLI_KVM_ENCODER_FRAME_SOCKET: files.frames,
      BLI_KVM_ENCODER_FRAME_USER: os.userInfo().username,
      BLI_KVM_ENCODER_REPORT: files.report,
      BLI_KVM_ENCODER_STATUS: files.status,
      BLI_KVM_VP8_DEMAND_SOCKET: files.demand,
      BLI_KVM_VP8_DEMAND_GROUP: os.userInfo().username,
      BLI_KVM_VP8_THREADS: "1",
    },
    "the encoder",
  );
  processes.push(encoder);
  const port = await freePort();
  const server = run(
    binary("BLI_KVM_SERVER", "bli-kvm-server"),
    {
      BLI_KVM_BIND: `127.0.0.1:${port}`,
      BLI_KVM_WEB_ROOT: path.resolve("web/dist"),
      BLI_KVM_STATE_DIR: path.join(directory, "state"),
      BLI_KVM_CONTROL_SOCKET: files.control,
      BLI_KVM_RESTRICTED_CONTROLS: "1",
      BLI_KVM_QUIC_LINK_SOCKET: files.link,
      BLI_KVM_QUIC_LINK_USER: os.userInfo().username,
      // H.264 over QUIC holds the H.264 encoder's demand like a WebRTC
      // viewer; no Janus is asked for anything.
      BLI_KVM_JANUS_WS: "127.0.0.1:8188",
      BLI_KVM_WEBRTC_H264_DEMAND_SOCKET: files.demand,
      BLI_KVM_H264_ENCODER_REPORT: files.report,
      BLI_KVM_H264_ENCODER_STATUS: files.status,
    },
    "the server",
  );
  processes.push(server);
  await server.said(/listening on/);
  const gateway = run(
    binary("BLI_KVM_QUIC_GATEWAY", "bli-kvm-quic-gateway"),
    {
      BLI_KVM_QUIC_BIND: `127.0.0.1:${port}`,
      BLI_KVM_QUIC_LINK_SOCKET: files.link,
      BLI_KVM_QUIC_FRAME_SOCKET: files.frames,
    },
    "the gateway",
  );
  processes.push(gateway);
  await gateway.said(/the server link is up/);

  browser = await launchChromium();
  const origin = `http://127.0.0.1:${port}`;
  const context = await browser.newContext({
    viewport: { width: 1280, height: 720 },
  });
  await context.addInitScript(
    ([keys, values]) => {
      for (const [index, key] of keys.entries())
        if (localStorage.getItem(key) === null)
          localStorage.setItem(key, values[index]);
    },
    [
      [CHOICE_KEY, VIDEO_MODE_KEY, "hardened-blikvm-menu-auto-hide"],
      ["quic", "h264", "always"],
    ],
  );
  const page = await context.newPage();
  page.setDefaultTimeout(PATIENCE_MS);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(`${origin}/`, { waitUntil: "load" });
  const account = { username: "operator", password: "gate-password-C4" };
  const signedIn = await page.evaluate(async (body) => {
    const post = (route) =>
      fetch(route, {
        method: "POST",
        credentials: "include",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
    await post("/api/auth/setup");
    return (await post("/api/auth/login")).status;
  }, account);
  assert.equal(signedIn, 200);
  await page.goto(`${origin}/`, { waitUntil: "load" });

  // The picture comes over the session.
  const deadline = now() + 30000;
  for (;;) {
    const frames = await page.evaluate(
      () =>
        document.querySelector('canvas#screen[data-video-transport="quic"]')
          ?.dataset.quicFrames,
    );
    if (Number(frames) >= 60) break;
    assert.ok(
      now() < deadline,
      `no picture over QUIC: ${frames}\n${server.log.slice(-2000)}`,
    );
    await sleep(200);
  }
  tap = tapFrames(files.frames);

  // The monitor, open in Basic.
  await page.getByTestId("quality-monitor-toggle").click();
  await page.getByTestId("quality-monitor").waitFor();
  await sleep(6000);
  const probeBefore = await videoProbe(page);
  const tapBefore = { bytes: tap.bytes, frames: tap.frames, at: now() };
  const shown = [];
  const began = now();
  while (now() - began < SPAN_MS) {
    await sleep(1000);
    shown.push(await rows(page));
  }
  const probeAfter = await videoProbe(page);
  const tapAfter = { bytes: tap.bytes, frames: tap.frames, at: now() };
  const seconds = (tapAfter.at - tapBefore.at) / 1000;
  const counted = {
    framesPerSecond: (probeAfter.frames - probeBefore.frames) / seconds,
    megabitsPerSecond: ((tapAfter.bytes - tapBefore.bytes) * 8) / 1e6 / seconds,
    ageMedianMs: medianAge(probeBefore, probeAfter),
    encodedPerSecond: (tapAfter.frames - tapBefore.frames) / seconds,
  };
  const monitor = {
    framesPerSecond: mean(shown.map((row) => Number.parseFloat(row.fps))),
    megabitsPerSecond: mean(shown.map((row) => Number.parseFloat(row.speed))),
    // The worker's median of its last five seconds, at the span's end.
    ageMedianMs: Number.parseFloat(shown.at(-1).delay),
    path: shown.at(-1).path,
    codec: shown.at(-1).codec,
  };
  receipt.counted = Object.fromEntries(
    Object.entries(counted).map(([key, value]) => [key, round(value)]),
  );
  receipt.monitor = {
    ...monitor,
    framesPerSecond: round(monitor.framesPerSecond),
    megabitsPerSecond: round(monitor.megabitsPerSecond, 3),
  };
  const within = (shownValue, countedValue, share) =>
    Math.abs(shownValue - countedValue) <= share * countedValue;
  receipt.checks = {
    framesPerSecondWithin10: within(
      monitor.framesPerSecond,
      counted.framesPerSecond,
      0.1,
    ),
    bytesRateWithin10: within(
      monitor.megabitsPerSecond,
      counted.megabitsPerSecond,
      0.1,
    ),
    delayWithin10Ms: Math.abs(monitor.ageMedianMs - counted.ageMedianMs) <= 10,
    path: monitor.path === "QUIC",
  };

  // The board's side, from the encoder's real report.
  await page.locator('[data-qm-level="extended"]').click();
  await sleep(2500);
  const extended = await rows(page);
  receipt.board = {
    encoder: extended.encoder,
    encodedFps: extended.encoderFps,
    stages: extended.stages,
    gate: extended.gate,
    quicRoundTrip: extended.quicRoundTrip,
    quicGateway: extended.quicGateway,
    decoder: extended.decoder,
    decodeTime: extended.decodeTime,
  };
  // x264 encodes on the host; the engine is chosen, and there is none.
  receipt.checks.boardFromReport =
    extended.encoder?.startsWith("x264") &&
    Number.parseFloat(extended.encoderFps) > 0;

  // What the open monitor costs the page's main thread, from Chromium's
  // trace of eight-second windows with the monitor closed and open in turn:
  // the difference of the whole thread's task time, in the median of the
  // pairs (a window's garbage collection of the video path varies by up to
  // two milliseconds a second); beside it what the monitor's own events
  // took: its timer (the page's count of its picture, sent to the
  // monitor's worker), messages, and the rendering of its window.
  const cdp = await context.newCDPSession(page);
  const RENDER = [
    "UpdateLayoutTree",
    "Layout",
    "PrePaint",
    "Paint",
    "Layerize",
    "Commit",
    "UpdateLayer",
    "HitTest",
  ];
  const traceWindow = async (open, level) => {
    const isOpen = (await page.getByTestId("quality-monitor").count()) > 0;
    if (isOpen !== open)
      await page.getByTestId("quality-monitor-toggle").click();
    if (open) await page.locator(`[data-qm-level="${level}"]`).click();
    // The pointer rests where it moves nothing.
    await page.mouse.move(1200, 700);
    await sleep(2500);
    const events = [];
    const collect = (data) => events.push(...data.value);
    cdp.on("Tracing.dataCollected", collect);
    const done = new Promise((resolve) =>
      cdp.once("Tracing.tracingComplete", resolve),
    );
    await cdp.send("Tracing.start", {
      categories: "devtools.timeline,disabled-by-default-devtools.timeline,v8",
      transferMode: "ReportEvents",
    });
    const began = now();
    await sleep(COST_WINDOW_MS);
    await cdp.send("Tracing.end");
    await done;
    cdp.off("Tracing.dataCollected", collect);
    const seconds = (now() - began) / 1000;
    const main = events.find(
      (event) =>
        event.name === "thread_name" && event.args?.name === "CrRendererMain",
    );
    const mine = events.filter(
      (event) =>
        main &&
        event.pid === main.pid &&
        event.tid === main.tid &&
        event.ph === "X",
    );
    const sum = (names) =>
      mine
        .filter((event) => names.includes(event.name))
        .reduce((total, event) => total + (event.dur ?? 0), 0) /
      1000 /
      seconds;
    return {
      timer: sum(["TimerFire"]),
      postMessage: sum(["HandlePostMessage"]),
      render: sum(RENDER),
      gc: sum(["MajorGC", "MinorGC", "V8.GC_MC_INCREMENTAL"]),
      task: sum(["RunTask"]),
      // The longest task, to tell a window that something else held.
      longest:
        Math.max(
          0,
          ...mine
            .filter((event) => event.name === "RunTask")
            .map((event) => event.dur ?? 0),
        ) / 1000,
    };
  };
  const median = (values) => {
    const sorted = [...values].sort((left, right) => left - right);
    const middle = Math.floor(sorted.length / 2);
    return sorted.length % 2
      ? sorted[middle]
      : (sorted[middle - 1] + sorted[middle]) / 2;
  };
  receipt.mainThreadMsPerSecond = {};
  for (const level of ["basic", "extended"]) {
    const pairs = [];
    for (let pair = 0; pair < COST_PAIRS; pair += 1) {
      const closed = await traceWindow(false);
      const open = await traceWindow(true, level);
      pairs.push({
        cost: round(open.task - closed.task),
        timer: round(open.timer - closed.timer),
        messages: round(open.postMessage - closed.postMessage),
        render: round(open.render),
        renderClosed: round(closed.render),
        gcOpen: round(open.gc),
        gcClosed: round(closed.gc),
        longestOpenMs: round(open.longest),
        longestClosedMs: round(closed.longest),
      });
    }
    receipt.mainThreadMsPerSecond[level] = {
      medianCost: round(median(pairs.map((pair) => pair.cost))),
      pairs,
    };
  }
  receipt.checks.costUnder2Ms =
    receipt.mainThreadMsPerSecond.basic.medianCost < 2 &&
    receipt.mainThreadMsPerSecond.extended.medianCost < 2;
  receipt.errors = errors;
  result =
    Object.values(receipt.checks).every(Boolean) && errors.length === 0
      ? "passed"
      : "failed";
} catch (error) {
  receipt.error = String(error?.message ?? error).slice(0, 2000);
} finally {
  receipt.result = result;
  tap?.connection?.destroy();
  if (browser) await browser.close();
  for (const process_ of processes.reverse()) await process_.stop();
  await owner?.close();
  await new Promise((resolve) => (camera ? camera.close(resolve) : resolve()));
  fs.rmSync(directory, { recursive: true, force: true });
  fs.writeFileSync(OUTPUT, `${JSON.stringify(receipt, null, 1)}\n`);
  console.log(JSON.stringify(receipt, null, 1));
}
process.exit(result === "passed" ? 0 : 1);
