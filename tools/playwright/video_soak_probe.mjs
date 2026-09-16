import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { observeMjpegProgress } from "./mjpeg_progress.mjs";

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
assert.ok(
  durationValue === undefined ||
    (/^[0-9]{1,4}$/.test(durationValue) &&
      Number(durationValue) >= 10 &&
      Number(durationValue) <= 86400),
  "invalid duration",
);
assert.equal(new URL(baseUrl).protocol, "https:");

const durationSeconds = Number(durationValue ?? 90);
const intervalMs = 5000;

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

const samples = [];
let phase = "launch";
const { chromium } = await import(pathToFileURL(modulePath).href);
const browser = await chromium.launch({ executablePath, headless: true });

function write(value) {
  fs.writeFileSync(outputPath, `${JSON.stringify(value, null, 2)}\n`, {
    encoding: "utf8",
    mode: 0o600,
  });
}

try {
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    locale: "en-US",
  });
  const page = await context.newPage();
  const observer = await observeMjpegProgress(page);

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
  await page.waitForFunction(() => {
    const screen = document.querySelector("img#screen");
    return screen instanceof HTMLImageElement && screen.naturalWidth > 0;
  });

  phase = "soak";
  let checkpoint = observer.checkpoint();
  const startedAt = Date.now();
  while (Date.now() - startedAt < durationSeconds * 1000) {
    const sampleStartedAt = Date.now();
    // `progress` resolves as soon as new bytes arrive and fails when the
    // stream stalls for the interval, so the explicit wait keeps one sample
    // per interval instead of one per network chunk (a fast MJPEG stream
    // would otherwise write tens of thousands of samples into the receipt).
    await observer.progress(checkpoint, intervalMs);
    checkpoint = observer.checkpoint();
    samples.push({
      elapsedSeconds: Math.round((Date.now() - startedAt) / 1000),
      bytes: checkpoint.bytes,
    });
    const remainingMs = intervalMs - (Date.now() - sampleStartedAt);
    if (remainingMs > 0) {
      await new Promise((resolve) => setTimeout(resolve, remainingMs));
    }
  }

  write({
    result: "passed",
    phase: "complete",
    durationSeconds,
    sampleCount: samples.length,
    finalBytes: checkpoint.bytes,
    samples,
  });
} catch (error) {
  write({
    result: "failed",
    phase,
    error: error?.name || "Error",
    samples,
  });
  process.exitCode = 1;
} finally {
  password = "";
  await browser.close();
}
