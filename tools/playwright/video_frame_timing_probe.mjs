import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [
  modulePath,
  executablePath,
  baseUrl,
  usernameFile,
  outputValue,
  durationValue,
] = process.argv.slice(2);
assert.ok(
  modulePath && executablePath && baseUrl && usernameFile && outputValue,
  "missing arguments",
);
const base = new URL(baseUrl);
assert.ok(
  base.protocol === "https:" ||
    ((base.hostname === "127.0.0.1" || base.hostname === "localhost") &&
      base.protocol === "http:"),
  "the stand must be reached over https (http is allowed only for a local mock)",
);

const durationSeconds = Number(durationValue ?? 10);
assert.ok(
  Number.isFinite(durationSeconds) &&
    durationSeconds >= 2 &&
    durationSeconds <= 60,
  "invalid duration",
);

const username = fs.readFileSync(usernameFile, "utf8").trim();
let password = "";
for await (const chunk of process.stdin) {
  password += chunk.toString("utf8");
  assert.ok(password.length <= 4096, "password input exceeded limit");
}
password = password.replace(/[\r\n]+$/, "");
assert.ok(password.length >= 8, "invalid password input");

const outputPath = path.resolve(outputValue);
assert.equal(path.dirname(outputPath), path.resolve("evidence/private"));

const { chromium } = await import(pathToFileURL(modulePath).href);
const browser = await chromium.launch({ executablePath, headless: true });

function write(value) {
  fs.writeFileSync(outputPath, `${JSON.stringify(value, null, 2)}\n`, {
    encoding: "utf8",
    mode: 0o600,
  });
}

let phase = "launch";
let measurement = null;

try {
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    locale: "en-US",
  });
  const page = await context.newPage();

  phase = "login";
  await page.goto(`${baseUrl}/#/auth/login`, { waitUntil: "networkidle" });
  await page.waitForFunction(
    () => document.title === "Login - Hardened BliKVM",
  );
  const inputs = page.locator("input");
  await inputs.nth(0).fill(username);
  await inputs.nth(1).fill(password);
  await page.getByRole("button", { name: "Login" }).click();
  await page.waitForURL(`${baseUrl}/#/`);
  await page.waitForFunction(
    () => document.title === "Remote Desktop - Hardened BliKVM",
  );

  phase = "measure";
  measurement = await page.evaluate(async (seconds) => {
    const controller = new AbortController();
    const timer = setTimeout(
      () => controller.abort(),
      Math.round(seconds * 1000),
    );
    const response = await fetch("/api/stream/mjpeg", {
      signal: controller.signal,
    });
    if (!response.ok || !response.body) {
      clearTimeout(timer);
      return { error: `stream status ${response.status}` };
    }
    const reader = response.body.getReader();
    const timestamps = [];
    let totalBytes = 0;
    let frameBytes = 0;
    // Kept across chunks so a JPEG marker that straddles two reads is seen.
    let previous = 0;
    try {
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        totalBytes += value.length;
        frameBytes += value.length;
        for (const byte of value) {
          previous = ((previous << 8) | byte) & 0xffff;
          if (previous === 0xffd9) {
            timestamps.push(performance.now());
            frameBytes = 0;
          }
        }
      }
    } catch (error) {
      if (error?.name !== "AbortError") {
        return { error: error?.name || "stream error" };
      }
    } finally {
      clearTimeout(timer);
      await reader.cancel().catch(() => undefined);
    }
    const intervals = [];
    for (let index = 1; index < timestamps.length; index += 1) {
      intervals.push(timestamps[index] - timestamps[index - 1]);
    }
    intervals.sort((left, right) => left - right);
    const pick = (fraction) =>
      intervals.length === 0
        ? 0
        : intervals[
            Math.min(
              intervals.length - 1,
              Math.floor(intervals.length * fraction),
            )
          ];
    const mean =
      intervals.length === 0
        ? 0
        : intervals.reduce((sum, value) => sum + value, 0) / intervals.length;
    return {
      frameCount: timestamps.length,
      totalBytes,
      sampledSeconds: seconds,
      averageFrameIntervalMs: Number(mean.toFixed(2)),
      p95FrameIntervalMs: Number(pick(0.95).toFixed(2)),
      maxFrameIntervalMs: Number(pick(1).toFixed(2)),
      averageFps: mean > 0 ? Number((1000 / mean).toFixed(2)) : 0,
      averageFrameBytes:
        timestamps.length === 0
          ? 0
          : Math.round(totalBytes / timestamps.length),
      truncationTailBytes: frameBytes,
    };
  }, durationSeconds);

  if (measurement.error) {
    throw new Error(measurement.error);
  }
  assert.ok(measurement.frameCount >= 30, "not enough complete frames");
  assert.ok(measurement.averageFrameBytes > 0, "frames carry no data");
  assert.ok(measurement.averageFrameIntervalMs > 0, "frame timing is invalid");

  write({
    probe: "hardened-blikvm-video-frame-timing",
    result: "passed",
    phase: "measure",
    ...measurement,
  });
  console.log(
    `video_frame_timing frames=${measurement.frameCount} fps=${measurement.averageFps} ` +
      `meanMs=${measurement.averageFrameIntervalMs} p95Ms=${measurement.p95FrameIntervalMs}`,
  );
} catch (error) {
  write({
    probe: "hardened-blikvm-video-frame-timing",
    result: "failed",
    phase,
    error: error?.name || "Error",
    message: String(error?.message ?? "").slice(0, 120),
    measurement,
  });
  process.exitCode = 1;
} finally {
  password = "";
  await browser.close();
}
