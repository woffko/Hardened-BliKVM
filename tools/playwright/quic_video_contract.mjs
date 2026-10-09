// The picture over the QUIC transport, through the built page, the host's
// build of the gateway, a played server (quic_fixture.mjs) and the encoder's
// own x264 engine behind a frame socket (the encoder crate's
// h264_frame_source). A UDP relay (tools/net/udp_impair_proxy.mjs) stands
// between the page and the gateway, and loses datagrams when told to.
//
// With QUIC chosen and H.264 the Screen menu's mode, headless Chromium
// decodes the stream with WebCodecs: the frame's number is read back from
// the canvas. Under loss the transport sends again what the path lost: the
// picture goes on, never shows an older frame after a newer one, and gives
// up and asks for nothing. Where the picture cannot be had over the session,
// H.264 stays on WebRTC and Settings says why. In the Restricted firewall
// mode, which closes WebRTC's ports and admits the session's, H.264 is to be
// had over the session and nothing else is. With upscaling on, the picture
// is drawn through the page's upscaler and stays readable, of the same
// levels, sharper than the browser's stretch, and the pointer lands where it
// is on the picture.
import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { startUdpProxy } from "../net/udp_impair_proxy.mjs";
import { startApp } from "./app_harness.mjs";
import {
  Board,
  connected,
  darkSquareLevel,
  frameNumber,
  launch,
  now,
  openPage,
  openSettings,
  sleep,
  squareEdgeRise,
  startFrameSource,
  until,
  VIDEO_MODE_KEY,
  videoProbe,
} from "./quic_fixture.mjs";
import { freePort } from "./quic_stack_rig.mjs";

/** The source's rate, and the least that counts as the whole picture. */
const FPS = 30;
const WHOLE_FPS = 24;
/** Each way, while the path is bad. */
const LOSS = 0.03;
const LOSSY_MS = 8000;

const browser = await launch();
const app = await startApp();
const PORT = Number(new URL(app.baseUrl).port);
const directory = fs.mkdtempSync(path.join(os.tmpdir(), "quic-video-"));
const frameSocket = path.join(directory, "frames.sock");
let source = await startFrameSource(frameSocket, { fps: FPS });
// The page reaches the gateway on its own port, where the relay listens.
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
// The menu bar stays where it is: this contract opens it late in a page's
// life, and is not about its hiding.
const MENU_KEY = "hardened-blikvm-menu-auto-hide";
const open = ({ prepare, ...options } = {}) =>
  openPage(browser, app, board, {
    choice: "quic",
    videoMode: "h264",
    ...options,
    prepare: async (context) => {
      await context.addInitScript(
        ([key, value]) => localStorage.setItem(key, value),
        [MENU_KEY, "always"],
      );
      if (prepare) await prepare(context);
    },
  });

/**
 * The picture's counters as its canvas carries them; `null` while H.264 is
 * not drawn from the session.
 */
const stats = (page) =>
  page.evaluate(() => {
    const canvas = document.querySelector(
      'canvas#screen[data-video-transport="quic"]',
    );
    if (!canvas || canvas.dataset.quicFrames === undefined) return null;
    const number = (value) => (value ? Number(value) : null);
    return {
      frames: Number(canvas.dataset.quicFrames),
      fps: Number(canvas.dataset.quicFps),
      ageMedian: number(canvas.dataset.quicAgeMedian),
      ageP95: number(canvas.dataset.quicAgeP95),
      waitMedian: number(canvas.dataset.quicWaitMedian),
      dropped: Number(canvas.dataset.quicDropped),
      requests: Number(canvas.dataset.quicRequests),
      resets: Number(canvas.dataset.quicResets),
    };
  });
const playing = (page, frames = 45) =>
  until("the picture comes over the session", async () => {
    const seen = await stats(page);
    return seen && seen.frames >= frames ? seen : null;
  });
/** WebRTC serves H.264: its video element is the screen. */
const onWebRtc = async (page) => {
  await page.locator("video#screen").waitFor({ state: "attached" });
  assert.equal(await stats(page), null);
};
const videoLine = (page) => page.locator('[data-testid="transport-video"]');
const videoLineSays = (page, state) =>
  page.waitForFunction(
    (wanted) =>
      document
        .querySelector('[data-testid="transport-video"]')
        ?.getAttribute("data-video") === wanted,
    state,
  );
async function closeSettings(page) {
  await page.keyboard.press("Escape");
  await page
    .locator('[data-testid="transport-settings"]')
    .waitFor({ state: "detached" });
}
/** What the Screen menu calls its H.264 mode. */
async function menuEntry(page) {
  await page.locator("svg.lucide-monitor").first().click();
  await page.getByText("Video Mode", { exact: true }).hover();
  const entry = page.locator('[data-video-mode-name="h264"]');
  await entry.waitFor();
  const text = await entry.innerText();
  // A click on the screen closes the menu; remote control is off.
  await page.mouse.click(640, 560);
  await entry.waitFor({ state: "hidden" });
  return text;
}
/**
 * The Screen menu's two WebRTC entries: what each is called, whether it can
 * be chosen, and what it says under the pointer when it cannot.
 */
async function menuModes(page) {
  await page.locator("svg.lucide-monitor").first().click();
  await page.getByText("Video Mode", { exact: true }).hover();
  const modes = {};
  for (const mode of ["h264", "vp8"]) {
    const entry = page.locator(`[data-video-mode="${mode}"]`);
    await entry.waitFor();
    modes[mode] = {
      name: await entry.locator("[data-video-mode-name]").innerText(),
      disabled: (await entry.getAttribute("aria-disabled")) === "true",
      says: null,
    };
    if (modes[mode].disabled) {
      await entry.hover();
      const tip = page.locator(".ant-tooltip:not(.ant-tooltip-hidden)");
      await tip.first().waitFor();
      modes[mode].says = await tip.first().innerText();
      // Away from the entry, so the next one's tip is the only one shown.
      await page.getByText("Video Mode", { exact: true }).hover();
      await tip.first().waitFor({ state: "hidden" });
    }
  }
  return modes;
}
/** The firewall status as the page reads it, in a mode. */
const firewall = (mode) => (context) =>
  context.route("**/api/system/firewall**", (route) =>
    route.fulfill({
      json: {
        code: 0,
        data: {
          effectiveMode: mode,
          restrictedActive: mode === "restricted",
          paranoidActive: mode === "paranoid",
          webrtcBlocked: mode === "restricted" || mode === "paranoid",
          quicBlocked: mode === "paranoid",
        },
      },
    }),
  );
const rememberedMode = (page) =>
  page.evaluate((key) => localStorage.getItem(key), VIDEO_MODE_KEY);
const subscriptions = () =>
  board.videoRequests.filter((request) => request.subscribe);
// Ends a scenario: the page leaves, as a closed tab does, and what its
// sessions held is given back.
async function finish(page, context) {
  await page.goto("about:blank");
  await until("the page's sessions are closed", () =>
    board.sessions.length === 0 && board.watching.size === 0 ? true : null,
  );
  await context.close();
  await sleep(100);
  board.videoRequests.length = 0;
}

try {
  // The picture comes over the session and is the encoder's: the frame's
  // number moves on at the source's rate, nothing is lost or asked for, and
  // the page says what serves the picture.
  {
    const { context, page, errors } = await open();
    const first = await playing(page);
    assert.deepEqual(subscriptions(), [
      { session: board.sessions[0], subscribe: true },
    ]);
    assert.equal(board.watching.size, 1);
    assert.equal(await page.locator("video#screen").count(), 0);

    const numberBefore = await frameNumber(page);
    const readAt = now();
    await sleep(2000);
    const numberAfter = await frameNumber(page);
    const seconds = (now() - readAt) / 1000;
    assert.ok(numberBefore !== null && numberAfter !== null, "no picture");
    const advanced = numberAfter - numberBefore;
    assert.ok(
      advanced >= seconds * FPS * 0.7 && advanced <= seconds * FPS * 1.3,
      `the picture moved ${advanced} frames in ${seconds.toFixed(2)} s`,
    );

    // The samples mean what the capture's JPEG means, the full range: the
    // source's dark squares (Y 16) are drawn at 16, not stretched to black.
    const dark = await darkSquareLevel(page);
    assert.ok(
      dark !== null && Math.abs(dark - 16) <= 4,
      `the dark squares are drawn at ${dark?.toFixed(1)}, not 16`,
    );

    const later = await stats(page);
    assert.ok(later.frames > first.frames);
    assert.ok(later.fps >= WHOLE_FPS, `${later.fps} frames a second`);
    assert.equal(later.requests, 0);
    assert.equal(later.dropped, 0);
    assert.equal(later.resets, 0);
    // A frame's age by the board's clock: there is one, and it is short.
    assert.ok(
      later.ageMedian !== null &&
        later.ageMedian > -5 &&
        later.ageMedian < 500 &&
        later.ageP95 >= later.ageMedian,
      `frame age ${later.ageMedian} ms, ${later.ageP95} ms`,
    );

    // What the worker counts for the probes: an age for the frames that a
    // ping has enclosed, the page's own delay for every frame, and the
    // capture time of the newest, which moves on with the board's clock.
    const sum = (counts) => counts.reduce((total, count) => total + count, 0);
    const probed = await videoProbe(page);
    await sleep(1000);
    const probedLater = await videoProbe(page);
    // The canvas says once a second what the worker counts all the time.
    const drawn = (await stats(page)).frames + 2 * FPS;
    assert.ok(probedLater.frames > probed.frames);
    assert.equal(sum(probedLater.waits), probedLater.frames);
    assert.ok(
      sum(probedLater.waits) >= later.frames && sum(probedLater.waits) <= drawn,
      `${sum(probedLater.waits)} delays for ${drawn} frames`,
    );
    assert.ok(
      sum(probedLater.ages) >= later.frames - FPS &&
        sum(probedLater.ages) <= drawn,
      `${sum(probedLater.ages)} ages for ${drawn} frames`,
    );
    const captured =
      (probedLater.lastCaptureMicros - probed.lastCaptureMicros) / 1e6;
    assert.ok(
      captured > 0.7 && captured < 1.6,
      `the newest frame was captured ${captured} s after the one a second ago`,
    );
    // The page adds little: a frame is drawn within milliseconds of arriving.
    assert.ok(
      later.waitMedian !== null &&
        later.waitMedian >= 0 &&
        later.waitMedian < 100,
      `${later.waitMedian} ms from arrival to drawing`,
    );

    assert.equal(await menuEntry(page), "H.264 (QUIC)");
    await openSettings(page);
    await connected(page);
    // The gateway's own count of this session's frames.
    const status = page.locator('[data-testid="transport-status"]');
    await page.waitForFunction(
      () =>
        Number(
          document
            .querySelector('[data-testid="transport-status"]')
            ?.getAttribute("data-frames-sent"),
        ) > 60,
    );
    assert.equal(await status.getAttribute("data-frames-dropped"), "0");
    assert.ok(Number(await status.getAttribute("data-key-frames")) >= 1);
    await videoLineSays(page, "quic");
    assert.match(
      await videoLine(page).innerText(),
      /^H\.264 video is using this session: 640x360, \d+(\.\d)? frames a second, a frame is \d+ ms old when it is decoded\.$/,
    );
    assert.equal(
      await page.locator('[data-testid="transport-video-retry"]').count(),
      0,
    );

    // The browser's choice back to Standard: the session ends, what the
    // server held for it is given back, and WebRTC serves H.264.
    await page
      .locator('[data-testid="transport-choice"] label', {
        hasText: "Standard",
      })
      .click();
    await onWebRtc(page);
    await until("the session is closed", () =>
      board.sessions.length === 0 && board.watching.size === 0 ? true : null,
    );
    assert.equal(await videoLine(page).count(), 0);
    await closeSettings(page);
    assert.equal(await menuEntry(page), "H.264 (WebRTC)");
    assert.deepEqual(errors, []);
    await finish(page, context);
  }

  // Upscaling (docs/design/screen-upscaling-v1.md). In a browser that draws
  // with a GPU (its WebGL renderer named as one) the picture shown at 200 %:
  // without upscaling the browser stretches the frame's canvas; with Auto the
  // worker draws through WebGL2 into a canvas of the shown size. The frame's
  // number reads and moves on, the dark squares are still drawn at 16, the
  // squares' edges rise at least a quarter more narrowly than the browser's
  // stretch, and the pointer lands where it is on the picture, from a mouse
  // and from a finger.
  {
    const GPU =
      "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 (0x00002503) Direct3D11 vs_5_0 ps_5_0, D3D11)";
    const zoomed = (upscale) => (context) =>
      context.addInitScript(
        ([gpu, upscale]) => {
          localStorage.setItem("hardened-blikvm-video-scale", "2");
          localStorage.setItem(
            "hardened-blikvm-upscale",
            // The picture at the size of a target as large as it (its own),
            // times the zoom: shown at 200 %, as without Fill.
            JSON.stringify({
              mode: upscale,
              sharpness: "medium",
              size: "target",
              target: { width: 640, height: 360 },
            }),
          );
          const getParameter = WebGLRenderingContext.prototype.getParameter;
          WebGLRenderingContext.prototype.getParameter = function (name) {
            // UNMASKED_RENDERER_WEBGL and RENDERER.
            if (name === 0x9246 || name === 0x1f01) return gpu;
            return getParameter.call(this, name);
          };
        },
        [GPU, upscale],
      );
    // The QUIC view's canvas as it says it draws; `mode` is null while
    // there is none, as when the picture is not drawn from the session.
    const drawnAs = (page) =>
      page.evaluate(() => {
        const canvas = document.querySelector(
          'canvas#screen[data-video-transport="quic"]',
        );
        return {
          mode: canvas?.dataset.quicUpscale ?? null,
          buffer: canvas?.dataset.quicBuffer ?? null,
          picture: canvas
            ? `${canvas.dataset.pictureWidth}x${canvas.dataset.pictureHeight}`
            : null,
        };
      });
    /**
     * The squares' edge rise and dark level over a few frames, to step over
     * frames with few boundaries.
     */
    const squares = async (page) => {
      const seen = [];
      for (let attempt = 0; attempt < 8 && seen.length < 3; attempt += 1) {
        const value = await squareEdgeRise(page, 2);
        if (value?.dark !== null && value !== null) seen.push(value);
        await sleep(150);
      }
      assert.ok(seen.length, "no square boundary seen");
      const mean = (pick) =>
        seen.reduce((sum, value) => sum + pick(value), 0) / seen.length;
      return { rise: mean((v) => v.rise), dark: mean((v) => v.dark) };
    };

    let plainRise;
    {
      const { context, page, errors } = await open({ prepare: zoomed("off") });
      await playing(page);
      await until("the plain picture reports itself", async () =>
        (await drawnAs(page)).mode === "off" ? true : null,
      );
      assert.equal((await drawnAs(page)).buffer, "640x360");
      plainRise = (await squares(page)).rise;
      assert.deepEqual(errors, []);
      await finish(page, context);
    }

    {
      const { context, page, errors } = await open({ prepare: zoomed("auto") });
      await playing(page);
      await until("the picture is upscaled", async () =>
        (await drawnAs(page)).mode === "medium" ? true : null,
      );
      assert.deepEqual(await drawnAs(page), {
        mode: "medium",
        buffer: "1280x720",
        picture: "640x360",
      });
      const before = await frameNumber(page, 2);
      await sleep(1000);
      const after = await frameNumber(page, 2);
      assert.ok(
        before !== null && after !== null && after > before,
        `the upscaled picture's number went from ${before} to ${after}`,
      );
      const upscaled = await squares(page);
      assert.ok(
        Math.abs(upscaled.dark - 16) <= 4,
        `the upscaled dark squares are drawn at ${upscaled.dark.toFixed(1)}, not 16`,
      );
      const upscaledRise = upscaled.rise;
      assert.ok(
        upscaledRise <= plainRise * 0.75,
        `the squares' edges rise in ${upscaledRise.toFixed(2)} px upscaled, ${plainRise.toFixed(2)} px stretched`,
      );
      const seen = await stats(page);
      assert.equal(seen.dropped, 0);
      assert.equal(seen.resets, 0);
      // The watchdog's count, as the monitor shows it: every frame that came
      // drawn through the upscaler.
      const [drawnFrames, cameFrames] = await page.evaluate(() =>
        document
          .querySelector("canvas#screen")
          .dataset.quicUpscaleFrames.split("/")
          .map(Number),
      );
      assert.ok(
        cameFrames >= 20 && drawnFrames >= cameFrames * 0.9,
        `drawn ${drawnFrames} of ${cameFrames} a second`,
      );

      // The pointer: the mouse at a point of the picture. Without the
      // board's power control the page sends the report over its WebSocket
      // at once (with it, only once remote control is enabled).
      assert.deepEqual(errors, []);
      await finish(page, context);
    }
    {
      board.powerControl = false;
      const sent = [];
      const { context, page, errors } = await open({
        prepare: async (context) => {
          await zoomed("auto")(context);
          await context.routeWebSocket("**/api/ws", (socket) =>
            socket.onMessage((message) => {
              if (typeof message !== "string") sent.push([...message]);
            }),
          );
        },
      });
      await playing(page);
      await until("the picture is upscaled", async () =>
        (await drawnAs(page)).mode === "medium" ? true : null,
      );
      const box = await page.locator("canvas#screen").boundingBox();
      const point = { x: 201, y: 401 };
      await page.mouse.move(box.x + point.x, box.y + point.y);
      // MessageEvent.Mouse, then the absolute report: buttons, x, y, wheel.
      const report = await until(
        "the mouse's report is sent",
        () => sent.filter((data) => data[0] === 2).at(-1) ?? null,
      );
      const at = (low, high) => report[low] | (report[high] << 8);
      assert.equal(at(2, 3), Math.round((0x7fff * point.x) / box.width));
      assert.equal(at(4, 5), Math.round((0x7fff * point.y) / box.height));
      board.powerControl = true;
      assert.deepEqual(errors, []);
      await finish(page, context);
      board.leases.length = 0;
    }
    // A finger on a phone: the cursor moves by the finger's way over the
    // picture as it is laid out, whatever the canvas holds.
    {
      const { context, page, errors } = await open({
        mobile: true,
        prepare: zoomed("auto"),
      });
      await playing(page);
      await until("the phone's picture is upscaled", async () =>
        (await drawnAs(page)).mode === "medium" ? true : null,
      );
      const touch = await context.newCDPSession(page);
      const drag = async (distance) => {
        const box = await page.locator("canvas#screen").boundingBox();
        const view = await page
          .locator("[data-kvm-screen-scroll]")
          .boundingBox();
        const x = view.x + view.width / 2 - distance / 2;
        const y = view.y + view.height / 2;
        await touch.send("Input.dispatchTouchEvent", {
          type: "touchStart",
          touchPoints: [{ x, y }],
        });
        for (let step = 1; step <= 10; step += 1) {
          await touch.send("Input.dispatchTouchEvent", {
            type: "touchMove",
            touchPoints: [{ x: x + (distance * step) / 10, y }],
          });
        }
        await touch.send("Input.dispatchTouchEvent", {
          type: "touchEnd",
          touchPoints: [],
        });
        return box.width;
      };
      // Where the cursor is on the picture, as a share of its width.
      const cursor = () =>
        page.evaluate(() => {
          const screen = document
            .getElementById("screen")
            .getBoundingClientRect();
          const mark = document.querySelector('[data-testid="mobile-cursor"]');
          return (
            (Number.parseFloat(mark.style.left) - screen.left) / screen.width
          );
        });
      await drag(-40);
      await sleep(200);
      const from = await cursor();
      const width = await drag(60);
      await sleep(200);
      const to = await cursor();
      assert.ok(
        Math.abs((to - from) * width - 60) <= 2,
        `a finger moved 60 px, the cursor ${((to - from) * width).toFixed(1)} px`,
      );

      // A pinch: the canvas keeps its buffer while the fingers move (the
      // browser stretches it), and is drawn at the picture's new size once
      // they have stopped for a moment. The fingers part by 3 px a step,
      // every step a larger zoom, short of the touch zoom's 400 %.
      const before = (await drawnAs(page)).buffer;
      const view = await page.locator("[data-kvm-screen-scroll]").boundingBox();
      const [cx, cy] = [view.x + view.width / 2, view.y + view.height / 2];
      const fingers = (spread) => [
        { x: cx - spread, y: cy, id: 1 },
        { x: cx + spread, y: cy, id: 2 },
      ];
      await touch.send("Input.dispatchTouchEvent", {
        type: "touchStart",
        touchPoints: fingers(30),
      });
      const during = new Set();
      for (let step = 1; step <= 16; step += 1) {
        await touch.send("Input.dispatchTouchEvent", {
          type: "touchMove",
          touchPoints: fingers(30 + step * 1.5),
        });
        during.add((await drawnAs(page)).buffer);
        await sleep(80);
      }
      await touch.send("Input.dispatchTouchEvent", {
        type: "touchEnd",
        touchPoints: [],
      });
      assert.deepEqual(
        [...during],
        [before],
        "the buffer followed the fingers",
      );
      const after = await until(
        "the buffer follows the pinch's final size",
        async () => {
          const box = await page.locator("canvas#screen").boundingBox();
          const shown = `${Math.round(box.width)}x${Math.round(box.height)}`;
          const { buffer } = await drawnAs(page);
          return buffer === shown && buffer !== before ? buffer : null;
        },
      );
      console.log(
        `QUIC upscaled on a phone: a pinch kept the ${before} buffer while the fingers moved and drew at ${after} once they stopped`,
      );
      assert.deepEqual(errors, []);
      await finish(page, context);
    }

    // Switched in the Screen menu on a live page: the view mounts a canvas
    // and a worker of the other kind, and the picture stays on the session.
    // (The gateway answers the old worker's unsubscription with "stopped",
    // which once came after the new one had subscribed and gave the picture
    // up to WebRTC.)
    {
      const { context, page, errors } = await open({ prepare: zoomed("auto") });
      await playing(page);
      await until("the picture is upscaled", async () =>
        (await drawnAs(page)).mode === "medium" ? true : null,
      );
      await page.locator("svg.lucide-monitor").first().click();
      for (const [entry, mode] of [
        ["off", "off"],
        ["auto", "medium"],
        ["off", "off"],
      ]) {
        await page.getByText("Upscaling", { exact: true }).hover();
        await page.locator(`[data-upscale-entry="${entry}"]`).click();
        await until(`the picture is drawn ${mode}`, async () =>
          (await drawnAs(page)).mode === mode ? true : null,
        );
        // The worker reports its count once a second, on a timer that its
        // draws can hold back a little: the picture goes on within three.
        const first = await stats(page);
        await until(
          `the picture goes on, drawn ${mode}`,
          async () => ((await stats(page)).frames > first.frames ? true : null),
          3000,
        );
      }
      assert.equal(await page.getByTestId("transport-notice").count(), 0);
      assert.deepEqual(errors, []);
      await finish(page, context);
    }

    // A hidden page (another tab, a phone's screen off): its picture is
    // decoded and not drawn, for longer than a stall takes, H.264 stays on
    // the session, and once the page is seen again the picture goes on,
    // upscaled, the watchdog counting afresh.
    {
      const { context, page, errors } = await open({ prepare: zoomed("auto") });
      await playing(page);
      await until("the picture is upscaled", async () =>
        (await drawnAs(page)).mode === "medium" ? true : null,
      );
      const hide = (hidden) =>
        page.evaluate((hidden) => {
          Object.defineProperty(document, "hidden", {
            configurable: true,
            get: () => hidden,
          });
          Object.defineProperty(document, "visibilityState", {
            configurable: true,
            get: () => (hidden ? "hidden" : "visible"),
          });
          document.dispatchEvent(new Event("visibilitychange"));
        }, hidden);
      await hide(true);
      await sleep(1500);
      const hidden = await stats(page);
      await sleep(11000);
      const still = await stats(page);
      assert.equal(still.frames, hidden.frames, "drawn while hidden");
      assert.equal(await page.locator("video#screen").count(), 0);
      await hide(false);
      await until("the picture goes on once the page is seen", async () =>
        (await stats(page)).frames > still.frames + 30 ? true : null,
      );
      assert.equal((await drawnAs(page)).mode, "medium");
      await sleep(6000);
      assert.equal(await page.getByTestId("upscale-notice").count(), 0);
      assert.deepEqual(errors, []);
      await finish(page, context);
    }

    // The watchdog. A path that loses datagrams loses frames to the network,
    // not to the upscaler: upscaling goes on. A draw slowed down in the
    // worker loses frames to the upscaler: within seconds upscaling stops
    // for the page load with a notice, and the picture goes on plainly.
    {
      const { context, page, errors } = await open({ prepare: zoomed("auto") });
      await playing(page);
      await until("the picture is upscaled", async () =>
        (await drawnAs(page)).mode === "medium" ? true : null,
      );
      const before = await stats(page);
      const lostBefore = relay.counters().toClient.dropped;
      relay.impair({ loss: LOSS });
      await sleep(LOSSY_MS);
      relay.impair({ loss: 0 });
      await sleep(1500);
      const lossy = await stats(page);
      assert.ok(
        relay.counters().toClient.dropped > lostBefore,
        "the path lost nothing",
      );
      // The transport sends what the path lost again: the picture goes on.
      assert.ok(
        lossy.frames - before.frames >= (LOSSY_MS / 1000) * WHOLE_FPS,
        `${lossy.frames - before.frames} frames drawn over the lossy path`,
      );
      assert.equal((await drawnAs(page)).mode, "medium");
      assert.equal(await page.getByTestId("upscale-notice").count(), 0);

      const worker = page
        .workers()
        .find((one) => one.url().includes("video.worker"));
      assert.ok(worker, "the picture's worker is not found");
      await worker.evaluate(() => {
        const draw = WebGL2RenderingContext.prototype.drawArrays;
        WebGL2RenderingContext.prototype.drawArrays = function (...args) {
          const until = performance.now() + 120;
          while (performance.now() < until);
          return draw.apply(this, args);
        };
      });
      const slowed = now();
      await page.getByTestId("upscale-notice").waitFor();
      const stoppedAfter = now() - slowed;
      assert.ok(stoppedAfter < 8000, `stopped after ${stoppedAfter} ms`);
      await until("the plain picture reports itself", async () =>
        (await drawnAs(page)).mode === "off" ? true : null,
      );
      const plainNumber = await frameNumber(page, 2);
      await sleep(1000);
      const laterNumber = await frameNumber(page, 2);
      assert.ok(
        plainNumber !== null && laterNumber > plainNumber,
        `the plain picture's number went from ${plainNumber} to ${laterNumber}`,
      );
      assert.equal(
        await page.evaluate(
          () =>
            document.querySelector('[data-testid="upscale-notice"]')
              ?.textContent,
        ),
        "Upscaling stopped on this page: frames were lost to it.",
      );
      assert.deepEqual(errors, []);
      await finish(page, context);
      console.log(
        `QUIC upscaled: a lossy path left upscaling on (${lossy.frames - before.frames} frames drawn over it); a slowed draw stopped it after ${(stoppedAfter / 1000).toFixed(1)} s`,
      );
    }
  }

  // A path that loses packets: the transport sends again what it lost, a
  // group of pictures on its stream a round trip late, so the picture goes
  // on at its rate and never steps back, no frame is given up and no key
  // frame asked for, and once the path is clean again the picture is whole.
  {
    const { context, page, errors } = await open();
    const before = await playing(page, 60);
    assert.equal(before.requests, 0);
    const lostBefore = relay.counters().toClient.dropped;
    const numbers = [];
    relay.impair({ loss: LOSS });
    const started = now();
    while (now() - started < LOSSY_MS) {
      numbers.push(await frameNumber(page));
      await sleep(200);
    }
    relay.impair({ loss: 0 });
    const atEnd = await stats(page);
    const lost = relay.counters().toClient.dropped - lostBefore;
    // What the loss broke last is still being mended: the picture is whole
    // once a second passes at the full rate with nothing given up or asked
    // for.
    let previous = null;
    const whole = await until("the picture is whole again", async () => {
      await sleep(1100);
      const seen = await stats(page);
      const calm =
        previous !== null &&
        seen.fps >= WHOLE_FPS &&
        seen.requests === previous.requests &&
        seen.dropped === previous.dropped;
      previous = seen;
      return calm ? seen : null;
    });
    const seconds = (now() - started) / 1000;

    assert.ok(lost > 0, "the relay lost nothing");
    // Every picture drawn was a whole frame, and none older than the last.
    assert.ok(
      numbers.every((number) => number !== null),
      `a picture was not a frame: ${numbers}`,
    );
    assert.deepEqual(
      numbers,
      [...numbers].sort((left, right) => left - right),
      "the picture stepped back",
    );
    assert.ok(
      numbers.at(-1) - numbers[0] >= (LOSSY_MS / 1000) * FPS * 0.5,
      `the picture stood still: ${numbers}`,
    );
    assert.ok(
      atEnd.frames - before.frames >= (LOSSY_MS / 1000) * WHOLE_FPS,
      `${atEnd.frames - before.frames} frames were drawn under loss`,
    );
    assert.equal(whole.requests, 0, "key frames were asked for");
    assert.equal(whole.dropped, 0, "frames were given up");
    console.log(
      `${LOSS * 100}% loss for ${(LOSSY_MS / 1000).toFixed(0)} s: ` +
        `${lost} packets lost towards the page, ` +
        `${atEnd.frames - before.frames} frames drawn, ` +
        `${whole.dropped} given up, ${whole.requests} key-frame requests, ` +
        `whole and calm again ${(seconds - LOSSY_MS / 1000).toFixed(1)} s after it`,
    );

    // Clean again: nothing more is given up or asked for.
    await sleep(2500);
    const settled = await stats(page);
    assert.equal(settled.requests, whole.requests);
    assert.equal(settled.dropped, whole.dropped);
    assert.ok(settled.fps >= WHOLE_FPS, `${settled.fps} frames a second`);
    // The status line counts the page's own requests.
    await openSettings(page);
    await page.waitForFunction(
      (requests) =>
        document
          .querySelector('[data-testid="transport-status"]')
          ?.textContent.endsWith(`key-frame requests ${requests}.`),
      settled.requests,
    );
    assert.deepEqual(errors, []);
    await finish(page, context);
  }

  // The gateway is started again: the page opens a new session, asks for
  // the picture on it, and the picture goes on.
  {
    const { context, page, errors } = await open();
    const before = await playing(page);
    const firstSession = board.sessions[0];
    await board.stopGateway();
    board.startGateway();
    await board.gatewayOnTheLink();
    await until("the picture is back on a new session", async () => {
      const seen = await stats(page);
      return seen &&
        seen.frames >= before.frames + 45 &&
        subscriptions().length === 2
        ? true
        : null;
    });
    assert.notEqual(board.sessions[0], firstSession);
    assert.equal(subscriptions()[1].session, board.sessions[0]);
    assert.equal(board.watching.size, 1);
    assert.equal(await page.locator("video#screen").count(), 0);
    assert.deepEqual(errors, []);
    await finish(page, context);
  }

  // The session is lost and does not come back within its attempts (the
  // gateway away for longer): H.264 goes on with WebRTC. With the gateway
  // back, the page tries again by itself once it is looked at again, and
  // the picture comes back over the new session.
  {
    const { context, page, errors } = await open();
    await playing(page);
    await board.stopGateway();
    await onWebRtc(page);
    board.startGateway();
    await board.gatewayOnTheLink();
    await page.evaluate(() =>
      document.dispatchEvent(new Event("visibilitychange")),
    );
    await playing(page);
    assert.equal(await page.locator("video#screen").count(), 0);
    assert.equal(board.watching.size, 1);
    assert.deepEqual(errors, []);
    await finish(page, context);
  }

  // The server has no H.264 for a QUIC session: H.264 stays on WebRTC,
  // Settings says so and offers another attempt, which succeeds once the
  // server has one.
  {
    board.videoRefusal = "unavailable";
    const { context, page, errors } = await open();
    await onWebRtc(page);
    assert.equal(board.watching.size, 0);
    assert.equal(await menuEntry(page), "H.264 (WebRTC)");
    await openSettings(page);
    // The session itself is open, and serves input.
    await connected(page);
    await videoLineSays(page, "webrtc");
    assert.equal(
      await videoLine(page).innerText(),
      "H.264 video stays on WebRTC: this device serves no H.264 over QUIC.",
    );
    board.videoRefusal = null;
    await page.locator('[data-testid="transport-video-retry"]').click();
    await videoLineSays(page, "quic");
    await playing(page);
    assert.equal(
      await page.locator('[data-testid="transport-video-retry"]').count(),
      0,
    );
    assert.equal(board.watching.size, 1);
    assert.deepEqual(errors, []);
    await finish(page, context);
  }

  // The encoder goes away: the gateway waits for it, then says the picture
  // has stopped, and lets the server's demand go. The page goes on with
  // WebRTC; with the encoder back, another attempt has the picture again.
  {
    const { context, page, errors } = await open();
    await playing(page);
    await source.stop();
    await onWebRtc(page);
    await until("the server's demand is let go", () =>
      board.watching.size === 0 ? true : null,
    );
    await openSettings(page);
    await videoLineSays(page, "webrtc");
    assert.equal(
      await videoLine(page).innerText(),
      "H.264 video stays on WebRTC: the H.264 encoder did not answer.",
    );
    source = await startFrameSource(frameSocket, { fps: FPS });
    await page.locator('[data-testid="transport-video-retry"]').click();
    await videoLineSays(page, "quic");
    const again = await playing(page);
    assert.ok(again.fps > 0);
    assert.notEqual(await frameNumber(page), null);
    assert.deepEqual(errors, []);
    await finish(page, context);
  }

  // An encoder that is there and sends nothing (nobody holds its demand, or
  // its capture gives nothing): after ten seconds without a frame the page
  // goes on with WebRTC and says why; with frames coming, another attempt
  // has the picture.
  {
    await source.stop();
    fs.rmSync(frameSocket, { force: true });
    const held = new Set();
    const silent = net.createServer((socket) => {
      held.add(socket);
      socket.on("error", () => undefined);
      socket.on("close", () => held.delete(socket));
    });
    await new Promise((resolve) => silent.listen(frameSocket, resolve));
    const { context, page, errors } = await open();
    await until("the gateway reaches the silent encoder", () =>
      held.size > 0 ? true : null,
    );
    await page
      .locator("video#screen")
      .waitFor({ state: "attached", timeout: 25000 });
    assert.equal(await stats(page), null);
    await openSettings(page);
    await connected(page);
    await videoLineSays(page, "webrtc");
    assert.equal(
      await videoLine(page).innerText(),
      "H.264 video stays on WebRTC: no frame came for ten seconds.",
    );
    for (const socket of held) socket.destroy();
    await new Promise((resolve) => silent.close(resolve));
    fs.rmSync(frameSocket, { force: true });
    source = await startFrameSource(frameSocket, { fps: FPS });
    await page.locator('[data-testid="transport-video-retry"]').click();
    await videoLineSays(page, "quic");
    await playing(page);
    assert.deepEqual(errors, []);
    await finish(page, context);
  }

  // A browser without an H.264 decoder in WebCodecs: the session is used,
  // the picture is not asked for over it, and Settings says why.
  {
    const { context, page, errors } = await open({
      prepare: (context) =>
        context.addInitScript(() => {
          delete window.VideoDecoder;
        }),
    });
    await onWebRtc(page);
    await openSettings(page);
    await connected(page);
    await videoLineSays(page, "decoder");
    assert.equal(
      await videoLine(page).innerText(),
      "This browser cannot decode H.264 with WebCodecs: H.264 video stays on WebRTC.",
    );
    assert.equal(
      await page.locator('[data-testid="transport-video-retry"]').count(),
      0,
    );
    await closeSettings(page);
    assert.equal(await menuEntry(page), "H.264 (WebRTC)");
    assert.deepEqual(board.videoRequests, []);
    assert.deepEqual(errors, []);
    await finish(page, context);
  }

  // The Restricted firewall mode closes WebRTC's ports and admits the
  // session's. With QUIC chosen the remembered H.264 mode stays and comes
  // over the session; VP8 cannot be had.
  {
    const { context, page, errors } = await open({
      prepare: firewall("restricted"),
    });
    await playing(page);
    assert.equal(await rememberedMode(page), "h264");
    const modes = await menuModes(page);
    assert.deepEqual(modes.h264, {
      name: "H.264 (QUIC)",
      disabled: false,
      says: null,
    });
    assert.equal(modes.vp8.disabled, true);
    assert.equal(
      modes.vp8.says,
      "VP8 WebRTC is disabled in Restricted and Paranoid Firewall modes",
    );
    assert.deepEqual(errors, []);
    await finish(page, context);
  }

  // The same mode, MJPEG on the screen: H.264 can be chosen, and is then
  // served over the session.
  {
    const { context, page, errors } = await open({
      videoMode: "mjpeg",
      prepare: firewall("restricted"),
    });
    await page.locator("img#screen").waitFor({ state: "attached" });
    await page.locator("svg.lucide-monitor").first().click();
    await page.getByText("Video Mode", { exact: true }).hover();
    const entry = page.locator('[data-video-mode="h264"]');
    await entry.waitFor();
    assert.equal(await entry.getAttribute("aria-disabled"), "false");
    assert.equal(
      await entry.locator("[data-video-mode-name]").innerText(),
      "H.264 (QUIC)",
    );
    await entry.click();
    // The page loads again with the new mode.
    await until("the page shows H.264 from the session", async () => {
      const seen = await stats(page).catch(() => null);
      return seen && seen.frames >= 15 ? true : null;
    });
    assert.equal(await rememberedMode(page), "h264");
    assert.deepEqual(errors, []);
    await finish(page, context);
  }

  // The same mode, a browser that uses Standard: H.264 is not to be had,
  // and its entry says what would give it.
  {
    const { context, page, errors } = await open({
      choice: "standard",
      videoMode: "mjpeg",
      prepare: firewall("restricted"),
    });
    await page.locator("img#screen").waitFor({ state: "attached" });
    const modes = await menuModes(page);
    assert.equal(modes.h264.name, "H.264 (WebRTC)");
    assert.equal(modes.h264.disabled, true);
    assert.equal(
      modes.h264.says,
      'H.264 WebRTC is disabled in the Restricted Firewall mode. With QUIC chosen in "Settings > System > Network", H.264 comes over QUIC instead.',
    );
    assert.equal(modes.vp8.disabled, true);
    assert.deepEqual(errors, []);
    await context.close();
  }

  // Paranoid closes the session's port as well. The page says that QUIC is
  // not to be had; the Screen menu, once opened, takes a remembered H.264
  // mode to MJPEG, and its entry promises nothing.
  {
    board.blocked = true;
    const { context, page, errors } = await open({
      prepare: firewall("paranoid"),
    });
    await page
      .locator('[data-testid="transport-notice"][data-reason="blocked"]')
      .waitFor();
    assert.equal(await stats(page), null);
    await page.locator("svg.lucide-monitor").first().click();
    await page.locator("img#screen").waitFor({ state: "attached" });
    assert.equal(await rememberedMode(page), "mjpeg");
    const modes = await menuModes(page);
    assert.equal(modes.h264.name, "H.264 (WebRTC)");
    assert.equal(modes.h264.disabled, true);
    assert.equal(
      modes.h264.says,
      "H.264 WebRTC is disabled in Restricted and Paranoid Firewall modes",
    );
    assert.deepEqual(board.videoRequests, []);
    board.blocked = false;
    assert.deepEqual(errors, []);
    await context.close();
  }

  // MJPEG keeps its path whatever the browser's choice.
  {
    const { context, page, errors } = await open({ videoMode: "mjpeg" });
    await page.locator("img#screen").waitFor({ state: "attached" });
    await openSettings(page);
    await connected(page);
    await videoLineSays(page, "other");
    assert.deepEqual(board.videoRequests, []);
    assert.deepEqual(errors, []);
    await finish(page, context);
  }

  console.log("quic video contract: ok");
} catch (error) {
  if (board.stderr) console.error(`gateway:\n${board.stderr}`);
  throw error;
} finally {
  await browser.close();
  await board.close();
  await relay.close();
  await source.stop();
  app.server.close();
  fs.rmSync(directory, { recursive: true, force: true });
}
