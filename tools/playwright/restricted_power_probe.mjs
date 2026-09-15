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
  // The extra Popconfirm would need an in-popup click; the probe disables it
  // through the same local-storage preference the settings toggle writes.
  await context.addInitScript(() =>
    localStorage.setItem("hardened-blikvm-power-confirm", "false"),
  );
  const page = await context.newPage();
  await page.setViewportSize({ width: 1600, height: 1000 });
  page.on("response", (response) => {
    const route = new URL(response.url()).pathname;
    if (route !== "/api/application-power" && route !== "/api/hid/focus")
      return;
    const event = {
      method: response.request().method(),
      route,
      status: response.status(),
      phase,
    };
    events.push(event);
    response
      .json()
      .then((body) => {
        event.applied = Boolean(body?.data?.applied);
        event.attested = Boolean(body?.data?.attested);
      })
      .catch(() => {
        event.bodyUnavailable = true;
      });
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

  phase = "open-power-menu";
  const confirmOverlay = page.getByRole("button", {
    name: "Confirm and control",
  });
  if ((await confirmOverlay.count()) > 0) {
    await confirmOverlay
      .first()
      .click({ force: true, timeout: 3000 })
      .catch(() => {});
    await page.waitForTimeout(500);
  }
  const viewport = page.viewportSize() ?? { width: 1280, height: 720 };
  await page.mouse.move(Math.round(viewport.width / 2), 30);
  await page.waitForTimeout(500);
  await page
    .locator("svg.lucide-power")
    .first()
    .click({ force: true, timeout: 5000 });
  const popconfirm = page.locator(".ant-popconfirm");
  // Geometry-free clicks: the menu panel can sit outside the headless
  // viewport, so dispatch the events on the real DOM nodes instead.
  const clicked = await page.evaluate(() => {
    const labels = Array.from(document.querySelectorAll("span, div"));
    const target = labels.find(
      (node) => (node.textContent || "").trim() === "Power (short click)",
    );
    const clickable = target?.closest("div[class*='cursor-pointer']") ?? target;
    clickable?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return Boolean(target);
  });
  assert.ok(clicked, "the power short entry was not found");

  phase = "confirm";
  await page.waitForTimeout(700);
  const confirmed = await page.evaluate(() => {
    const buttons = Array.from(document.querySelectorAll("button"));
    const target = buttons.find(
      (button) => (button.textContent || "").trim() === "Confirm and control",
    );
    if (!target) return false;
    target.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  });
  await page.waitForTimeout(2500);
  const popoverVisible = await popconfirm.count();
  const confirmationClicked = confirmed;

  const powerEvents = events.filter(
    (event) => event.route === "/api/application-power",
  );
  const focusEvents = events.filter(
    (event) => event.route === "/api/hid/focus",
  );
  const passed = powerEvents.some((event) => event.applied);
  write({
    probe: "hardened-blikvm-restricted-power",
    result: passed ? "passed" : "failed",
    phase,
    events,
    powerResponses: powerEvents.length,
    focusResponses: focusEvents.length,
    popoverVisible,
    confirmationClicked,
  });
  if (!passed) {
    process.exitCode = 1;
  }
} catch (error) {
  write({
    probe: "hardened-blikvm-restricted-power",
    result: "failed",
    phase,
    error: error?.name || "Error",
    message: String(error?.message ?? "").slice(0, 120),
    events,
  });
  process.exitCode = 1;
} finally {
  password = "";
  await browser.close();
}
