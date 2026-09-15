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
  imageName,
] = process.argv.slice(2);
assert.ok(
  modulePath &&
    executablePath &&
    baseUrl &&
    usernameFile &&
    outputValue &&
    imageName,
  "missing arguments",
);
const base = new URL(baseUrl);
assert.ok(
  base.protocol === "https:" ||
    ((base.hostname === "127.0.0.1" || base.hostname === "localhost") &&
      base.protocol === "http:"),
  "the stand must be reached over https (http is allowed only for a local mock)",
);
assert.match(imageName, /^[A-Za-z0-9._-]{1,64}$/, "invalid image name");

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

function mediaEvents() {
  return events.filter((event) => event.route === "/api/virtual-media");
}

function appliedCount(action, connected) {
  return mediaEvents().filter((event) => {
    if (!event.applied) return false;
    if (action === "select") return event.action === "select";
    return event.action === "set_connected" && event.connected === connected;
  }).length;
}

let page = null;
try {
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    locale: "en-US",
  });
  page = await context.newPage();
  page.on("response", (response) => {
    const route = new URL(response.url()).pathname;
    if (route !== "/api/virtual-media" && route !== "/api/auth/login") return;
    const request = response.request();
    const event = {
      method: request.method(),
      route,
      status: response.status(),
      phase,
    };
    events.push(event);
    response
      .json()
      .then((body) => {
        // The server wraps the result as { code, data: { applied, rejection } };
        // the fixture uses the flat shape.
        const result = body?.data ?? body;
        event.applied = Boolean(result?.applied);
        event.action = result?.command?.action;
        event.connected = result?.command?.connected;
        if (request.method() === "POST" && route === "/api/virtual-media") {
          try {
            const posted = request.postDataJSON();
            event.action = posted?.command?.action;
            event.connected = posted?.command?.connected;
          } catch {
            event.postDataUnavailable = true;
          }
        }
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

  phase = "open-media-menu";
  await page.locator("svg.lucide-disc").first().click({ force: true });
  const row = page.getByText(imageName, { exact: true });
  await row.waitFor({ state: "visible", timeout: 10000 });

  phase = "attach";
  await row.click();
  await page.waitForTimeout(12000);
  assert.ok(appliedCount("select") >= 1, "the image select was not applied");
  assert.ok(
    appliedCount("set_connected", true) >= 1,
    "the attach was not applied",
  );

  phase = "eject";
  await row.click();
  await page.waitForTimeout(6000);
  assert.ok(
    appliedCount("set_connected", false) >= 1,
    "the eject was not applied",
  );

  phase = "settle";
  await page.waitForTimeout(1000);
  write({
    probe: "hardened-blikvm-virtual-media-acceptance",
    result: "passed",
    imageName,
    phase,
    events,
    selectApplied: appliedCount("select"),
    attachApplied: appliedCount("set_connected", true),
    ejectApplied: appliedCount("set_connected", false),
  });
} catch (error) {
  let currentUrl = "unavailable";
  let currentTitle = "";
  try {
    if (page) {
      currentUrl = page.url();
      currentTitle = await page.title();
    }
  } catch {
    currentUrl = "unavailable";
  }
  write({
    probe: "hardened-blikvm-virtual-media-acceptance",
    result: "failed",
    imageName,
    phase,
    error: error?.name || "Error",
    currentUrl,
    currentTitle,
    events,
  });
  process.exitCode = 1;
} finally {
  password = "";
  await browser.close();
}
