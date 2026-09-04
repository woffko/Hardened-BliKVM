import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";

const [modulePath, executablePath, baseUrl] = process.argv.slice(2);
assert.ok(modulePath && executablePath && baseUrl, "missing browser arguments");
assert.equal(new URL(baseUrl).protocol, "https:");

let browser = null;
let phase = "launch";
const requests = [];
let webSockets = 0;

try {
  const { chromium } = await import(pathToFileURL(modulePath).href);
  browser = await chromium.launch({ executablePath, headless: true });
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    locale: "en-US",
  });
  const page = await context.newPage();
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.name));
  page.on("websocket", () => {
    webSockets += 1;
  });
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (path.startsWith("/api/")) {
      requests.push({ method: request.method(), path });
    }
  });

  phase = "document";
  const response = await page.goto(`${baseUrl}/#/auth/login`, {
    waitUntil: "networkidle",
  });
  assert.equal(response?.status(), 200);
  assert.equal(response?.headers()["x-content-type-options"], "nosniff");
  assert.equal(response?.headers()["x-frame-options"], "DENY");
  assert.equal(response?.headers()["referrer-policy"], "no-referrer");

  phase = "first-setup";
  await page.waitForFunction(
    () => document.title === "First Setup - Hardened BliKVM",
  );
  await page
    .getByRole("heading", { name: "Create administrator account" })
    .waitFor();
  assert.equal(await page.locator("input").count(), 3);
  await page.getByRole("button", { name: "Create account" }).waitFor();

  phase = "setup-api";
  const setup = await page.evaluate(async () => {
    const result = await fetch("/api/auth/setup", { credentials: "include" });
    return { status: result.status, body: await result.json() };
  });
  assert.equal(setup.status, 200);
  assert.equal(setup.body.code, 0);
  assert.equal(setup.body.data.required, true);

  phase = "invariants";
  const cookies = await context.cookies(baseUrl);
  assert.equal(cookies.length, 0);
  assert.deepEqual(pageErrors, []);
  assert.equal(webSockets, 0);
  assert.ok(requests.length >= 1);
  assert.equal(
    requests.every((item) => item.method === "GET"),
    true,
  );
  assert.equal(
    requests.some((item) => item.path.startsWith("/api/control")),
    false,
  );
  assert.equal(
    requests.some((item) => item.path === "/api/auth/setup"),
    true,
  );
  const bodyText = await page.locator("body").innerText();
  assert.doesNotMatch(bodyText, /PiKVM|NanoKVM/i);

  process.stdout.write(
    `${JSON.stringify({
      schemaVersion: 1,
      result: "passed",
      title: await page.title(),
      setupRequired: true,
      inputCount: 3,
      cookieCount: 0,
      apiRequests: requests.length,
      apiMethods: [...new Set(requests.map((item) => item.method))].sort(),
      controlRequests: 0,
      webSockets,
      legacyBrandingAbsent: true,
    })}\n`,
  );
} catch (error) {
  process.stdout.write(
    `${JSON.stringify({
      schemaVersion: 1,
      result: "failed",
      phase,
      errorType: error?.name || "Error",
      apiRequests: requests.length,
      controlRequests: requests.filter((item) =>
        item.path.startsWith("/api/control"),
      ).length,
      webSockets,
    })}\n`,
  );
  process.exitCode = 1;
} finally {
  if (browser) await browser.close();
}
