// A stand-in for the board's side of the H.264 engine choice and of the
// quality monitor (docs/design/h264-engine-choice-and-quality-monitor-v1.md):
// the fields `GET /api/vm/screen` gains, `POST /api/vm/screen` with
// `h264_engine`, and `GET /api/vm/quality`, for the page's contracts on the
// host. It is a test support module, never a shipped artifact.
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

/** Playwright's Chromium, where the installer puts it for this user. */
export async function launchChromium() {
  const { chromium } = await import(
    pathToFileURL(
      process.env.PLAYWRIGHT_MODULE ??
        path.join(
          os.homedir(),
          ".local/share/playwright/node_modules/playwright/index.mjs",
        ),
    ).href
  );
  return chromium.launch({
    headless: true,
    executablePath:
      process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ??
      path.join(
        os.homedir(),
        ".cache/ms-playwright/chromium-1228/chrome-linux64/chrome",
      ),
  });
}

/** The capabilities' video section of a board that offers the choice. */
export function engineChoiceVideo(overrides = {}) {
  const status = (availability) => ({
    availability,
    evidence: "source_verified",
  });
  return {
    status: status("read_write"),
    transports: ["mjpeg", "web_rtc_janus", "web_rtc_janus_h264"],
    preferredTransport: "mjpeg",
    maxWidth: null,
    maxHeight: null,
    audio: status("unavailable"),
    settings: ["fps", "resolution", "webrtc_bitrate", "h264_engine"],
    ...overrides,
  };
}

/**
 * The board as the stand-in keeps it. `engine` is the choice, `encoder`
 * what encodes, `hardwareAvailable` the encoder's report (`undefined`
 * where it has not reported).
 */
export function boardState(overrides = {}) {
  return {
    engine: "hardware",
    encoder: "hardware",
    hardwareAvailable: true,
    hardwareReason: null,
    kbps: 8000,
    fps: 60,
    capturedFps: 30,
    queuedFps: 30,
    width: 1920,
    height: 1080,
    jpegQuality: 100,
    ...overrides,
  };
}

/** What `GET /api/vm/screen` answers for `board`. */
export function screenState(board) {
  const data = {
    requestedResolution: `${board.width}x${board.height}`,
    applied: true,
    fps: board.fps,
    quality: board.jpegQuality,
    capturedFps: board.capturedFps,
    width: board.width,
    height: board.height,
    queuedFps: board.queuedFps,
    webrtcKbps: board.kbps,
    h264Engine: board.engine,
    h264Encoder: board.encoder,
  };
  if (board.hardwareAvailable !== undefined)
    data.h264HardwareAvailable = board.hardwareAvailable;
  if (board.hardwareAvailable === false && board.hardwareReason)
    data.h264HardwareReason = board.hardwareReason;
  return data;
}

/**
 * An encoder's report as `GET /api/vm/quality` gives it: checked, with
 * rates. `second` counts the stand-in's seconds, so that counters grow.
 */
export function encoderReport(board, codec, second = 0) {
  const hardware = codec === "h264" && board.encoder === "hardware";
  const fps = 30;
  return {
    schema: 1,
    codec,
    streaming: true,
    engine: codec === "vp8" ? "libvpx" : hardware ? "hardware" : board.encoder,
    engineWanted: codec === "h264" ? board.engine : null,
    hardwareAvailable:
      codec === "h264" ? (board.hardwareAvailable ?? null) : null,
    hardwareReason: codec === "h264" ? board.hardwareReason : null,
    width: board.width,
    height: board.height,
    kbps: board.kbps,
    maxFps: 30,
    converters: 3,
    encodedFrames: 1000 + second * fps,
    fps,
    stageMicros: {
      decode: 28100,
      convert: 9600,
      encode: hardware ? 17600 : 51000,
    },
    gate: {
      admitted: 1000 + second * fps,
      tooSoon: 500 + second * 30,
      unchanged: 20,
    },
    gatePerSecond: { admitted: fps, tooSoon: 30, unchanged: 0 },
    hardware: hardware
      ? {
          passes: 1300 + second * 38,
          bothSearches: 200 + second * 6,
          wide: 150,
          higherQp: 9,
          leftOut: 3,
          keyFrames: 40,
          failures: 0,
          engineMicros: (1300 + second * 38) * 7300,
          passesPerSecond: 38,
          engineMicrosPerPass: 7300,
        }
      : null,
  };
}

/** What `GET /api/vm/quality` answers for `board`. */
export function qualityState(board, second = 0, { vp8 = false } = {}) {
  return {
    capture: {
      width: board.width,
      height: board.height,
      fps: board.capturedFps,
      queuedFps: board.queuedFps,
      quality: board.jpegQuality,
    },
    h264: encoderReport(board, "h264", second),
    vp8: vp8 ? encoderReport(board, "vp8", second) : null,
    units: {
      h264CpuPercent: board.encoder === "hardware" ? 156 : 307,
      vp8CpuPercent: vp8 ? 120 : null,
    },
    temperatureMillidegrees: 68300,
    version: "0.1.0-fixture",
  };
}

/**
 * Answers the screen and the quality routes on a Playwright `page` from a
 * mutable `board`, records what was asked (a change with its setting and
 * value), and lets a test choose how a change of the engine is answered
 * (`refuse` names a rejection).
 */
export async function routeEngineAndQuality(page, board, options = {}) {
  const asked = [];
  let second = 0;
  await page.route("**/api/vm/screen", async (route) => {
    const request = route.request();
    if (request.method() === "POST") {
      const body = request.postDataJSON();
      asked.push(`POST /api/vm/screen ${body?.type}=${body?.value}`);
      if (body?.type === "h264_engine") {
        if (options.refuse) {
          await route.fulfill({
            json: {
              code: 0,
              data: { applied: false, rejection: options.refuse },
            },
          });
          return;
        }
        if (body.value !== 0 && body.value !== 1) {
          await route.fulfill({
            json: {
              code: 0,
              data: { applied: false, rejection: "value_out_of_range" },
            },
          });
          return;
        }
        board.engine = body.value === 1 ? "hardware" : "software";
        board.encoder =
          board.engine === "hardware" && board.hardwareAvailable !== false
            ? "hardware"
            : "x264";
        await route.fulfill({
          json: {
            code: 0,
            data: { applied: true, h264Engine: board.engine },
          },
        });
        return;
      }
    } else {
      asked.push(`${request.method()} /api/vm/screen`);
    }
    await route.fulfill({ json: { code: 0, data: screenState(board) } });
  });
  await page.route("**/api/vm/quality", async (route) => {
    asked.push(`${route.request().method()} /api/vm/quality`);
    second += 1;
    await route.fulfill({
      json: {
        code: 0,
        data: qualityState(board, second, { vp8: options.vp8 ?? false }),
      },
    });
  });
  return asked;
}
