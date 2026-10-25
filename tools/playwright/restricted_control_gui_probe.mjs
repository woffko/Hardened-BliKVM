import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

import { takeControl } from "./take_control.mjs";

const [
  modulePath,
  executablePath,
  baseUrl,
  usernameFile,
  outputValue,
  renewWaitValue,
] = process.argv.slice(2);
assert.ok(
  modulePath && executablePath && baseUrl && usernameFile && outputValue,
  "missing arguments",
);
assert.ok(
  renewWaitValue === undefined ||
    (/^[0-9]{1,3}$/.test(renewWaitValue) && Number(renewWaitValue) <= 60),
  "invalid renew wait",
);
const base = new URL(baseUrl);
assert.ok(
  base.protocol === "https:" ||
    ((base.hostname === "127.0.0.1" || base.hostname === "localhost") &&
      base.protocol === "http:"),
  "the stand must be reached over https (http is allowed only for a local mock)",
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

const events = [];
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
  page.on("response", (response) => {
    const route = new URL(response.url()).pathname;
    if (route.startsWith("/api/hid") || route === "/api/auth/login") {
      const record = {
        method: response.request().method(),
        route,
        status: response.status(),
        phase,
      };
      events.push(record);
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

  phase = "wait-screen";
  await page.waitForFunction(() => {
    const screen = document.querySelector("img#screen");
    return screen instanceof HTMLImageElement && screen.naturalWidth > 0;
  });

  phase = "enable-control";
  await takeControl(page);

  phase = "key";
  await page.keyboard.press("KeyA");

  phase = "settle";
  await page.waitForTimeout(1500);
  if (renewWaitValue) {
    phase = "renew";
    await page.waitForTimeout(Number(renewWaitValue) * 1000 - 1500);
  }

  const leaseResponses = events.filter((event) =>
    event.route.startsWith("/api/hid/lease"),
  );
  const expectedLeaseResponses = renewWaitValue ? 4 : 1;
  const passed = leaseResponses.length >= expectedLeaseResponses;
  write({
    result: passed ? "passed" : "failed",
    phase,
    events,
    leaseResponseCount: leaseResponses.length,
  });
  if (!passed) {
    process.exitCode = 1;
  }
} catch (error) {
  write({ result: "failed", phase, error: error?.name || "Error", events });
  process.exitCode = 1;
} finally {
  password = "";
  await browser.close();
}
