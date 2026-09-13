import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [modulePath, executablePath, baseUrl, usernameFile, outputValue] =
  process.argv.slice(2);
assert.ok(
  modulePath && executablePath && baseUrl && usernameFile && outputValue,
  "missing arguments",
);
assert.equal(new URL(baseUrl).protocol, "https:");

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

const streams = [];
let phase = "launch";
const { chromium } = await import(pathToFileURL(modulePath).href);
const browser = await chromium.launch({ executablePath, headless: true });

function write(value) {
  fs.writeFileSync(outputPath, `${JSON.stringify(value, null, 2)}\n`, {
    encoding: "utf8",
    mode: 0o600,
  });
}

async function waitForDecoded(page) {
  await page.waitForFunction(
    () => {
      const screen = document.querySelector("img#screen");
      return screen instanceof HTMLImageElement && screen.naturalWidth > 0;
    },
    { timeout: 20000 },
  );
}

function waitForStream(page) {
  return page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === "/api/stream/mjpeg" &&
      response.request().method() === "GET" &&
      response.status() === 200,
    { timeout: 20000 },
  );
}

try {
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    locale: "en-US",
  });
  const page = await context.newPage();
  page.on("response", (response) => {
    const route = new URL(response.url()).pathname;
    if (route === "/api/stream/mjpeg") {
      streams.push({ page: "primary", status: response.status() });
    }
  });

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

  phase = "primary-video";
  const primaryStream = waitForStream(page);
  assert.equal((await primaryStream).status(), 200);
  await waitForDecoded(page);

  phase = "second-client";
  const second = await context.newPage();
  second.on("response", (response) => {
    const route = new URL(response.url()).pathname;
    if (route === "/api/stream/mjpeg") {
      streams.push({ page: "second", status: response.status() });
    }
  });
  const secondStream = waitForStream(second);
  await second.goto(`${baseUrl}/#/`, { waitUntil: "domcontentloaded" });
  await second.waitForFunction(
    () => document.title === "Remote Desktop - Hardened BliKVM",
  );
  assert.equal((await secondStream).status(), 200);
  await waitForDecoded(second);

  phase = "both-live";
  const bothDecoded = await page.evaluate(() => {
    const screen = document.querySelector("img#screen");
    return screen instanceof HTMLImageElement && screen.naturalWidth > 0;
  });
  assert.ok(bothDecoded);

  phase = "settle";
  await page.waitForTimeout(3000);

  const passed = streams.length >= 2 && streams.every((s) => s.status === 200);
  write({
    result: passed ? "passed" : "failed",
    phase,
    streams,
    streamCount: streams.length,
  });
  if (!passed) {
    process.exitCode = 1;
  }
} catch (error) {
  write({ result: "failed", phase, error: error?.name || "Error", streams });
  process.exitCode = 1;
} finally {
  password = "";
  await browser.close();
}
