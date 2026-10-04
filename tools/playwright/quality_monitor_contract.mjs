// The quality monitor (docs/design/h264-engine-choice-and-quality-monitor-v1.md)
// against the stand-in for the board's side (quality_fixture.mjs): QM on the
// toolbar opens and closes it, in the desktop and the phone layout, and it
// can be hidden like the other items; its window lies over the picture,
// translucent, is dragged by its header, and in Extended is resized,
// scrolled and made opaque; the browser keeps its level, place, size and
// opacity. The rows of MJPEG, of H.264 and VP8 over WebRTC (a stand-in for
// the connection's statistics), H.264 by either engine, and Russian. The
// board is asked once a second while the monitor is open, and never while
// it is closed. The QUIC path's rows are measured against the real stack
// (quality_monitor_gate.mjs).
import assert from "node:assert/strict";
import { defaultManifest, startApp } from "./app_harness.mjs";
import {
  boardState,
  engineChoiceVideo,
  launchChromium,
  routeEngineAndQuality,
} from "./quality_fixture.mjs";

const browser = await launchChromium();
const app = await startApp();
const MODE_KEY = "hardened-blikvm-video-mode";
const LANGUAGE_KEY = "hardened-blikvm-language";
const MENU_KEY = "hardened-blikvm-menu-auto-hide";
const HIDDEN_KEY = "hardened-blikvm-menu-disabled-items";
const LAYOUT_KEY = "hardened-blikvm-quality-monitor";

/**
 * A stand-in for a WebRTC connection's statistics and its video element's
 * count of frames drawn: 1 Mbit/s, 30 frames a second, 50 ms in the jitter
 * buffer a frame, a round trip of 4 ms, 4 ms to decode a frame.
 */
function standInStatistics() {
  const began = performance.now();
  const seconds = () => (performance.now() - began) / 1000;
  RTCPeerConnection.prototype.getStats = async function getStats() {
    const t = seconds();
    const frames = Math.floor(30 * t);
    const stats = new Map();
    const add = (stat) => stats.set(stat.id, stat);
    add({ id: "T", type: "transport", selectedCandidatePairId: "P" });
    add({
      id: "P",
      type: "candidate-pair",
      state: "succeeded",
      nominated: true,
      currentRoundTripTime: 0.004,
      localCandidateId: "L",
      remoteCandidateId: "R",
    });
    add({ id: "L", type: "local-candidate", candidateType: "host" });
    add({ id: "R", type: "remote-candidate", candidateType: "host" });
    add({
      id: "V",
      type: "inbound-rtp",
      kind: "video",
      bytesReceived: Math.floor((1e6 / 8) * t),
      packetsReceived: Math.floor(100 * t),
      packetsLost: 3,
      jitter: 0.002,
      framesReceived: frames,
      framesDecoded: frames,
      framesDropped: 0,
      jitterBufferDelay: 0.05 * frames,
      jitterBufferEmittedCount: frames,
      totalDecodeTime: 0.004 * frames,
      decoderImplementation: "ExternalDecoder",
      powerEfficientDecoder: true,
      frameWidth: 1920,
      frameHeight: 1080,
      nackCount: 1,
      pliCount: 2,
      firCount: 0,
    });
    return stats;
  };
  HTMLVideoElement.prototype.getVideoPlaybackQuality = function quality() {
    return {
      totalVideoFrames: Math.floor(30 * seconds()),
      droppedVideoFrames: 0,
      creationTime: performance.now(),
    };
  };
}

/**
 * How many canvases for the monitor's rows the page has made: a loop of
 * mounting and failing shows here.
 */
function countMonitorCanvases() {
  window.__monitorCanvases = 0;
  const count = (node) =>
    node.nodeType === 1 &&
    (node.matches("canvas[data-testid='quality-monitor-rows']") ||
      node.querySelector("canvas[data-testid='quality-monitor-rows']"))
      ? 1
      : 0;
  new MutationObserver((records) => {
    for (const record of records)
      for (const node of record.addedNodes)
        window.__monitorCanvases += count(node);
  }).observe(document, { childList: true, subtree: true });
}

/**
 * A browser that cannot draw the rows in a worker: one without the hand-over
 * of a canvas to a worker, or one that refuses to make a worker.
 */
function withoutWorkers(kind) {
  if (kind === "offscreen") {
    delete HTMLCanvasElement.prototype.transferControlToOffscreen;
  } else {
    window.Worker = function Worker() {
      throw new DOMException("blocked by a policy", "SecurityError");
    };
  }
}

async function open({
  mode,
  board = boardState(),
  vp8 = false,
  language,
  hidden,
  viewport = { width: 1280, height: 720 },
  touch = false,
  layout,
  without,
} = {}) {
  const manifest = defaultManifest();
  manifest.video = { ...manifest.video, ...engineChoiceVideo() };
  const context = await browser.newContext({
    viewport,
    hasTouch: touch,
    isMobile: touch,
  });
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  await context.addInitScript(
    ([keys, values]) => {
      if (sessionStorage.getItem("fixture-set")) return;
      sessionStorage.setItem("fixture-set", "1");
      for (const [index, key] of keys.entries()) {
        if (values[index] !== null) localStorage.setItem(key, values[index]);
      }
    },
    [
      [MODE_KEY, MENU_KEY, LANGUAGE_KEY, HIDDEN_KEY, LAYOUT_KEY],
      [
        mode,
        "always",
        language ?? null,
        hidden ? JSON.stringify(hidden) : null,
        layout ? JSON.stringify(layout) : null,
      ],
    ],
  );
  await context.addInitScript(standInStatistics);
  await context.addInitScript(countMonitorCanvases);
  if (without) await context.addInitScript(withoutWorkers, without);
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("**/api/capabilities", (route) =>
    route.fulfill({ json: { code: 0, data: manifest } }),
  );
  await page.route("**/api/stream/mjpeg/detect", (route) =>
    route.fulfill({ json: { code: 0, data: { enabled: true } } }),
  );
  await page.route("**/api/system/firewall**", (route) =>
    route.fulfill({
      json: {
        code: 0,
        data: { effectiveMode: "baseline", webrtcBlocked: false },
      },
    }),
  );
  const asked = await routeEngineAndQuality(page, board, { vp8 });
  await page.goto(app.baseUrl, { waitUntil: "load" });
  return { context, page, errors, asked, board };
}

const monitor = (page) => page.getByTestId("quality-monitor");
const qualityAsked = (asked) =>
  asked.filter((line) => line === "GET /api/vm/quality").length;

/** What the monitor's worker drew last: every row's value by its key, the
 * labels, the sections, and the size of its canvas in device pixels. */
async function drawn(page) {
  for (const worker of page.workers()) {
    const found = await worker
      .evaluate(() =>
        self.qualityMonitorRows
          ? {
              rows: self.qualityMonitorRows,
              labels: self.qualityMonitorLabels,
              sections: self.qualityMonitorSections,
              pixels: self.qualityMonitorPixels,
            }
          : null,
      )
      .catch(() => null);
    if (found) return found;
  }
  return { rows: {}, labels: {}, sections: [], pixels: null };
}
const rows = async (page) => (await drawn(page)).rows;

/** Waits until the rows hold what `check` wants; returns them. */
async function rowsUntil(page, check, what) {
  const deadline = Date.now() + 8000;
  for (;;) {
    const shown = await rows(page);
    if (check(shown)) return shown;
    assert.ok(Date.now() < deadline, `${what}: ${JSON.stringify(shown)}`);
    await page.waitForTimeout(100);
  }
}

const box = (page) => monitor(page).boundingBox();

async function dragBy(page, locator, dx, dy) {
  const at = await locator.boundingBox();
  const x = at.x + Math.min(20, at.width / 2);
  const y = at.y + at.height / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  await page.mouse.move(x + dx / 2, y + dy / 2, { steps: 3 });
  await page.mouse.move(x + dx, y + dy, { steps: 3 });
  await page.mouse.up();
}

try {
  // MJPEG on the desktop: opened and closed by QM, asked only while open,
  // its Basic rows, Extended with its sections, dragged, resized, scrolled,
  // made opaque and remembered.
  {
    const { context, page, errors, asked } = await open({ mode: "mjpeg" });
    const toggle = page.getByTestId("quality-monitor-toggle");
    await toggle.waitFor();
    assert.equal(await monitor(page).count(), 0);
    await page.waitForTimeout(1500);
    assert.equal(qualityAsked(asked), 0, "asked while closed");
    await toggle.click();
    await monitor(page).waitFor();
    assert.equal(await monitor(page).getAttribute("data-level"), "basic");
    assert.equal(await monitor(page).getAttribute("data-opaque"), "false");
    const basic = await rowsUntil(
      page,
      (shown) => shown.bitrate === "JPEG 100",
      "MJPEG rows",
    );
    assert.deepEqual(basic, {
      speed: "-",
      delay: "-",
      path: "MJPEG",
      fps: "- / 30.0",
      bitrate: "JPEG 100",
      codec: "MJPEG 1920x1080",
      chroma: "4:2:2",
    });
    // Over the picture, in the top left corner at first.
    const first = await box(page);
    assert.ok(first.x < 40 && first.y < 120, JSON.stringify(first));
    await page.waitForTimeout(2600);
    const whileOpen = qualityAsked(asked);
    assert.ok(
      whileOpen >= 3 && whileOpen <= 5,
      `${whileOpen} asks in about 3 s`,
    );
    // The rows' canvas is mounted again once for the mode the page sets
    // after its load, and then not any more.
    assert.ok(
      (await page.evaluate(() => window.__monitorCanvases)) <= 2,
      "the monitor kept mounting canvases",
    );

    await page.locator('[data-qm-level="extended"]').click();
    await rowsUntil(
      page,
      (shown) => shown.version !== undefined,
      "Extended's sections",
    );
    const extended = await rowsUntil(
      page,
      (shown) => Boolean(shown.refresh) && shown.refresh !== "- Hz",
      "Extended rows",
    );
    assert.equal(extended.mjpegCapture, "30.0");
    assert.equal(extended.version, "0.1.0-fixture");
    assert.equal(extended.temperature, "68.3 °C");
    assert.match(extended.refresh, /^\d+ Hz$/);
    assert.deepEqual((await drawn(page)).sections, [
      "basic",
      "transport",
      "board",
      "local",
    ]);
    await page.getByTestId("quality-monitor-opaque").click();
    assert.equal(await monitor(page).getAttribute("data-opaque"), "true");
    // Dragged by its header, resized at its corner.
    await dragBy(page, page.getByTestId("quality-monitor-header"), 160, 90);
    const moved = await box(page);
    assert.ok(Math.abs(moved.x - first.x - 160) < 4, JSON.stringify(moved));
    assert.ok(Math.abs(moved.y - first.y - 90) < 4, JSON.stringify(moved));
    await dragBy(page, page.getByTestId("quality-monitor-resize"), -120, -260);
    const resized = await box(page);
    assert.ok(resized.width < moved.width - 100, JSON.stringify(resized));
    assert.ok(resized.height < moved.height - 200, JSON.stringify(resized));
    // Smaller than its rows: it scrolls.
    const scrolled = await page
      .getByTestId("quality-monitor-body")
      .evaluate((body) => {
        body.scrollTop = 40;
        return {
          top: body.scrollTop,
          overflows: body.scrollHeight > body.clientHeight,
        };
      });
    assert.ok(scrolled.overflows && scrolled.top > 0, JSON.stringify(scrolled));
    // The rows' clicks and drags reach nothing else on the page.
    await page.mouse.click(resized.x + resized.width / 2, resized.y + 40);

    // The browser keeps it: open, Extended, opaque, where it was, its size.
    await page.reload({ waitUntil: "load" });
    await monitor(page).waitFor();
    assert.equal(await monitor(page).getAttribute("data-level"), "extended");
    assert.equal(await monitor(page).getAttribute("data-opaque"), "true");
    const again = await box(page);
    for (const key of ["x", "y", "width", "height"]) {
      assert.ok(
        Math.abs(again[key] - resized[key]) < 2,
        `${key}: ${JSON.stringify(again)}`,
      );
    }
    await page.getByTestId("quality-monitor-close").click();
    await monitor(page).waitFor({ state: "detached" });
    const closedAt = qualityAsked(asked);
    await page.waitForTimeout(2200);
    assert.equal(qualityAsked(asked), closedAt, "asked after it was closed");
    await page.reload({ waitUntil: "load" });
    await page.waitForTimeout(800);
    assert.equal(await monitor(page).count(), 0, "it stays closed");
    assert.deepEqual(errors, []);
    await context.close();
  }

  // H.264 over WebRTC, by the engine: the page's own numbers from the
  // connection's statistics, the board's from its answer.
  {
    const { context, page, errors } = await open({
      mode: "h264",
      layout: {
        open: true,
        level: "extended",
        x: 12,
        y: 56,
        width: 380,
        height: 640,
      },
    });
    await monitor(page).waitFor();
    const shown = await rowsUntil(
      page,
      (current) =>
        Boolean(current.speed) &&
        current.speed !== "-" &&
        Boolean(current.decoder) &&
        current.decoder !== "-",
      "WebRTC rows",
    );
    const megabits = Number.parseFloat(shown.speed);
    assert.ok(Math.abs(megabits - 1) < 0.1, shown.speed);
    assert.match(shown.delay, /^~5[0-3] ms$/);
    assert.equal(shown.path, "WebRTC");
    const drawn = Number.parseFloat(shown.fps);
    assert.ok(Math.abs(drawn - 30) < 3, shown.fps);
    assert.equal(shown.bitrate, "8000 kbit/s");
    assert.equal(shown.codec, "H.264 HW 1920x1080");
    assert.equal(shown.chroma, "4:2:0");
    assert.equal(shown.rtcRoundTrip, "4.0 ms");
    assert.equal(shown.rtcCandidates, "host - host");
    assert.match(shown.rtcPackets, /^\d+ \/ 3$/);
    assert.equal(shown.rtcJitter, "2.0 ms");
    assert.equal(shown.rtcJitterBuffer, "50.0 ms");
    assert.equal(shown.rtcFeedback, "1 / 2 / 0");
    assert.equal(shown.encoder, "video engine");
    assert.equal(shown.enginePass, "7.3 ms, 1.27");
    assert.equal(shown.gate, "30.0 / 30.0 / 0.0");
    assert.equal(shown.unitCpu, "156 %");
    assert.equal(shown.decoder, "ExternalDecoder (hardware)");
    assert.equal(shown.decodeTime, "4.0 ms");
    // No MJPEG rows on this path.
    assert.equal(shown.mjpegCapture, undefined);
    assert.deepEqual(errors, []);
    await context.close();
  }

  // x264 encodes H.264, and VP8.
  {
    const { context, page, errors } = await open({
      mode: "h264",
      board: boardState({ engine: "software", encoder: "x264" }),
      layout: {
        open: true,
        level: "extended",
        x: 12,
        y: 56,
        width: 380,
        height: 640,
      },
    });
    await monitor(page).waitFor();
    const shown = await rowsUntil(
      page,
      (current) => Boolean(current.codec) && current.codec !== "H.264",
      "x264 rows",
    );
    assert.equal(shown.codec, "H.264 SW 1920x1080");
    assert.equal(shown.encoder, "x264");
    assert.equal(shown.enginePass, undefined, "no engine's rows for x264");
    assert.equal(shown.unitCpu, "307 %");
    assert.deepEqual(errors, []);
    await context.close();
  }
  {
    const { context, page, errors } = await open({
      mode: "vp8",
      vp8: true,
      layout: {
        open: true,
        level: "extended",
        x: 12,
        y: 56,
        width: 380,
        height: 640,
      },
    });
    await monitor(page).waitFor();
    const shown = await rowsUntil(
      page,
      (current) => Boolean(current.codec?.startsWith("VP8 ")),
      "VP8 rows",
    );
    assert.equal(shown.codec, "VP8 1920x1080");
    assert.equal(shown.encoder, "libvpx");
    assert.equal(shown.unitCpu, "120 %");
    assert.equal(shown.chroma, "4:2:0");
    assert.deepEqual(errors, []);
    await context.close();
  }

  // On a phone: QM is there, the window narrower and its text smaller;
  // hidden from the toolbar by the Appearance setting; Russian.
  {
    const { context, page, errors } = await open({
      mode: "mjpeg",
      viewport: { width: 390, height: 844 },
      touch: true,
    });
    await page.getByTestId("quality-monitor-toggle").tap();
    await monitor(page).waitFor();
    const narrow = await box(page);
    assert.ok(narrow.width <= 200, JSON.stringify(narrow));
    assert.equal(
      await monitor(page).evaluate(
        (element) => getComputedStyle(element).fontSize,
      ),
      "10px",
    );
    assert.deepEqual(errors, []);
    await context.close();
  }
  {
    const { context, page, errors } = await open({
      mode: "mjpeg",
      hidden: ["qualityMonitor"],
    });
    await page.locator("svg.lucide-monitor").first().waitFor();
    assert.equal(await page.getByTestId("quality-monitor-toggle").count(), 0);
    assert.deepEqual(errors, []);
    await context.close();
  }
  {
    const { context, page, errors } = await open({
      mode: "mjpeg",
      language: "ru",
      layout: {
        open: true,
        level: "basic",
        x: 12,
        y: 56,
        width: 340,
        height: 460,
      },
    });
    await monitor(page).waitFor();
    await rowsUntil(page, (shown) => shown.chroma === "4:2:2", "Russian rows");
    const { labels } = await drawn(page);
    assert.equal(labels.speed, "Скорость");
    assert.equal(labels.chroma, "Цветность");
    assert.equal(
      await page.locator('[data-qm-level="extended"]').innerText(),
      "Подробно",
    );
    assert.deepEqual(errors, []);
    await context.close();
  }

  // A browser that cannot draw in a worker (no hand-over of a canvas, or the
  // worker refused), with the monitor open from the stored layout: the window
  // says so in place of the rows and does nothing else: no loop of canvases,
  // no request to the board, no error screen; the page works, and QM closes
  // the window and opens it again.
  for (const without of ["offscreen", "worker"]) {
    const { context, page, errors, asked } = await open({
      mode: "mjpeg",
      without,
      layout: { open: true },
    });
    await page.waitForTimeout(1000);
    const noteAtOneSecond = await page
      .getByTestId("quality-monitor-unsupported")
      .count();
    await page.waitForTimeout(2000);
    const made = await page.evaluate(() => window.__monitorCanvases);
    assert.ok(made <= 1, `${without}: ${made} canvases made in three seconds`);
    assert.equal(noteAtOneSecond, 1, `${without}: no note within a second`);
    assert.equal(
      await page.getByTestId("quality-monitor-rows").count(),
      0,
      `${without}: a canvas is left`,
    );
    assert.equal(qualityAsked(asked), 0, `${without}: the board was asked`);
    assert.equal(
      await page.getByText("Unexpected Application Error").count(),
      0,
      `${without}: the page's error screen shows`,
    );
    await page.getByTestId("menu-bar").waitFor();
    await page.getByTestId("quality-monitor-close").click();
    await monitor(page).waitFor({ state: "detached" });
    await page.getByTestId("quality-monitor-toggle").click();
    await page.getByTestId("quality-monitor-unsupported").waitFor();
    assert.deepEqual(errors, [], without);
    await context.close();
  }

  // The page zoomed, or moved to a display of another pixel ratio: the rows
  // are drawn again at the new ratio and stay sharp.
  {
    const { context, page, errors } = await open({
      mode: "mjpeg",
      layout: { open: true },
    });
    await monitor(page).waitFor();
    const pixelsUntil = async (check, what) => {
      const deadline = Date.now() + 3000;
      for (;;) {
        const { pixels } = await drawn(page);
        if (pixels && check(pixels)) return pixels;
        assert.ok(Date.now() < deadline, `${what}: ${JSON.stringify(pixels)}`);
        await page.waitForTimeout(100);
      }
    };
    const first = await pixelsUntil((pixels) => pixels.width > 0, "no draw");
    const session = await context.newCDPSession(page);
    // As a zoom does, the ratio and the viewport's size change together; the
    // browser's emulation tells a page of the change of its pixel ratio only
    // then.
    await session.send("Emulation.setDeviceMetricsOverride", {
      width: 1200,
      height: 700,
      deviceScaleFactor: 2,
      mobile: false,
    });
    const sharp = await pixelsUntil(
      (pixels) => pixels.width === first.width * 2,
      `the rows were not drawn again at twice the pixels of ${first.width}`,
    );
    assert.equal(sharp.height, first.height * 2);
    assert.deepEqual(errors, []);
    await context.close();
  }

  console.log(
    "Quality monitor: QM opens and closes it on desktop and phone and can be hidden, dragged, resized, scrolled, opaque and remembered, MJPEG, WebRTC by either engine and VP8 rows, Russian, the board asked only while it is open, a browser without OffscreenCanvas or with the worker refused told so without a loop or an error screen, and the rows drawn again when the pixel ratio changes passed",
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
