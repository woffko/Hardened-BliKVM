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
      events.push({
        method: response.request().method(),
        route,
        status: response.status(),
        phase,
      });
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

  phase = "open-menu";
  await page.locator("svg.lucide-keyboard").first().click({ force: true });
  await page.getByText("On-screen keyboard", { exact: true }).click();

  phase = "click-key";
  await page
    .locator(".hg-button")
    .first()
    .waitFor({ state: "visible", timeout: 5000 });
  await page.locator(".hg-button").first().click();

  phase = "confirm";
  const confirm = page.getByRole("button", { name: "Confirm and control" });
  await confirm.waitFor({ state: "visible", timeout: 5000 });
  await confirm.click();

  phase = "settle";
  await page.waitForTimeout(1500);

  const leaseResponses = events.filter((event) =>
    event.route.startsWith("/api/hid/lease"),
  );
  const passed = leaseResponses.length >= 2;
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
