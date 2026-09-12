import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { chromium } from "/home/w0w/.local/share/playwright/node_modules/playwright/index.mjs";

// Only ephemeral synthetic credentials produced by the Rust fixture.
const config = JSON.parse(await readFile(process.argv[2], "utf8"));
const origin = new URL(config.origin);
assert.equal(origin.hostname, "127.0.0.1");
assert.equal(origin.protocol, "https:");
const browser = await chromium.launch({
  executablePath:
    "/home/w0w/.cache/ms-playwright/chromium-1228/chrome-linux64/chrome",
  headless: true,
});
try {
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
  const separator = config.cookie.indexOf("=");
  await context.addCookies([
    {
      name: config.cookie.slice(0, separator),
      value: config.cookie.slice(separator + 1),
      url: origin.origin,
      secure: true,
      httpOnly: true,
    },
  ]);
  const page = await context.newPage();
  await page.goto(`${origin.origin}/transport-probe`);
  const cdp = await context.newCDPSession(page);
  await cdp.send("Network.enable");
  const throttle = (uploadThroughput) =>
    cdp.send("Network.emulateNetworkConditions", {
      offline: false,
      latency: 0,
      downloadThroughput: -1,
      uploadThroughput,
    });
  await throttle(65536);
  await page.evaluate((csrf) => {
    window.progress = 0;
    window.uploadResult = null;
    const xhr = new XMLHttpRequest();
    xhr.open("POST", "/api/application-update/upload");
    xhr.setRequestHeader("Content-Type", "application/octet-stream");
    xhr.setRequestHeader("x-csrf-token", csrf);
    xhr.upload.onprogress = (event) => {
      window.progress = (event.loaded / event.total) * 100;
    };
    xhr.onload = () => (window.uploadResult = xhr.status);
    xhr.onerror = () => (window.uploadResult = "error");
    xhr.onabort = () => (window.uploadResult = "aborted");
    xhr.send(new Blob([new Uint8Array(8724480)]));
  }, config.csrf);
  await page.waitForFunction(
    () => window.progress >= 1 && window.progress < 100,
  );
  const progress = await page.evaluate(() => window.progress);
  const logoutStatus = await page.evaluate(async (csrf) => {
    const response = await fetch("/api/auth/logout", {
      method: "POST",
      headers: { "Content-Type": "application/json", "x-csrf-token": csrf },
      body: "{}",
    });
    return response.status;
  }, config.csrf);
  assert.equal(logoutStatus, 200);
  // This PID belongs exclusively to the Rust fixture's disposable nginx.
  // Queue upstream closure and resumed client data before its event loop runs.
  assert.ok(Number.isSafeInteger(config.proxyPid) && config.proxyPid > 1);
  process.kill(config.proxyPid, "SIGSTOP");
  try {
    await throttle(-1);
    await new Promise((resolve) => setTimeout(resolve, 150));
  } finally {
    process.kill(config.proxyPid, "SIGCONT");
  }
  await page.waitForFunction(() => window.uploadResult !== null);
  const uploadStatus = await page.evaluate(() => window.uploadResult);
  console.log(JSON.stringify({ progress, logoutStatus, uploadStatus }));
  assert.equal(uploadStatus, 401);
} finally {
  await browser.close();
}
