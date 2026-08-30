import assert from "node:assert/strict";
import crypto from "node:crypto";
import { pathToFileURL } from "node:url";

const [modulePath, executablePath, baseUrl] = process.argv.slice(2);
assert.ok(
  modulePath && executablePath && baseUrl,
  "missing browser runtime arguments",
);
assert.equal(new URL(baseUrl).protocol, "https:");

let phase = "browser-launch";
let browser = null;
try {
  const { chromium } = await import(pathToFileURL(modulePath).href);
  const password = `Tls-${crypto.randomBytes(24).toString("hex")}`;
  const changedPassword = `Rotated-${crypto.randomBytes(24).toString("hex")}`;
  const username = "tls-browser-admin";
  browser = await chromium.launch({ executablePath, headless: true });
  phase = "browser-context";
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    locale: "en-US",
  });
  const page = await context.newPage();
  const responses = [];
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(error.name));
  page.on("response", (response) => {
    const path = new URL(response.url()).pathname;
    if (path.startsWith("/api/")) {
      responses.push({
        method: response.request().method(),
        path,
        status: response.status(),
      });
    }
  });

  phase = "load-document";
  const documentResponse = await page.goto(`${baseUrl}/#/auth/login`, {
    waitUntil: "networkidle",
  });
  assert.equal(documentResponse?.status(), 200);
  assert.equal(new URL(page.url()).protocol, "https:");
  assert.equal(
    documentResponse?.headers()["x-content-type-options"],
    "nosniff",
  );
  phase = "first-setup";
  await page.waitForFunction(
    () => document.title === "First Setup - Hardened BliKVM",
  );
  await page
    .getByRole("heading", { name: "Create administrator account" })
    .waitFor();

  let inputs = page.locator("input");
  assert.equal(await inputs.count(), 3);
  await inputs.nth(0).fill(username);
  await inputs.nth(1).fill(password);
  await inputs.nth(2).fill(password);
  phase = "setup-submit";
  const setupResponse = page.waitForResponse(
    (response) =>
      response.url().endsWith("/api/auth/setup") &&
      response.request().method() === "POST",
  );
  const loginResponse = page.waitForResponse(
    (response) =>
      response.url().endsWith("/api/auth/login") &&
      response.request().method() === "POST",
  );
  await page.getByRole("button", { name: "Create account" }).click();
  assert.equal((await setupResponse).status(), 200);
  assert.equal((await loginResponse).status(), 200);
  phase = "post-login";
  await page.waitForURL(`${baseUrl}/#/`);
  await page.waitForLoadState("networkidle");
  await page.waitForFunction(
    () => document.title === "Adapter status - Hardened BliKVM",
  );

  phase = "cookies";
  let cookies = await context.cookies(baseUrl);
  let session = cookies.find(
    (cookie) => cookie.name === "hardened-blikvm-session",
  );
  let csrf = cookies.find((cookie) => cookie.name === "hardened-blikvm-csrf");
  assert.ok(session, "missing session cookie");
  assert.equal(session.httpOnly, true);
  assert.equal(session.secure, true);
  assert.equal(session.sameSite, "Strict");
  assert.ok(csrf, "missing CSRF cookie");
  assert.equal(csrf.httpOnly, false);
  assert.equal(csrf.secure, true);
  assert.equal(csrf.sameSite, "Strict");

  phase = "inventory";
  const inventory = await page.evaluate(async () => {
    const response = await fetch("/api/inventory", { credentials: "include" });
    return { status: response.status, body: await response.json() };
  });
  assert.equal(inventory.status, 200);
  assert.equal(inventory.body.code, 0);
  assert.equal(inventory.body.data.schemaVersion, 1);
  assert.equal(inventory.body.data.backend, "kvmd_transition");
  assert.doesNotMatch(
    JSON.stringify(inventory.body.data),
    /password|credential|username|serial|ssid|token|cookie|endpoint|socketPath|macAddress/i,
  );

  phase = "password-page";
  await page.goto(`${baseUrl}/#/auth/password`, { waitUntil: "networkidle" });
  await page.waitForFunction(
    () => document.title === "Change Password - Hardened BliKVM",
  );
  inputs = page.locator("input");
  assert.equal(await inputs.count(), 3);
  await inputs.nth(1).fill(changedPassword);
  await inputs.nth(2).fill(changedPassword);
  phase = "password-change";
  const passwordResponse = page.waitForResponse(
    (response) =>
      response.url().endsWith("/api/auth/password") &&
      response.request().method() === "POST",
  );
  await page.getByRole("button", { name: "Ok" }).click();
  assert.equal((await passwordResponse).status(), 200);
  await page.waitForURL(`${baseUrl}/#/auth/login`);
  cookies = await context.cookies(baseUrl);
  assert.equal(
    cookies.some((cookie) => cookie.name === "hardened-blikvm-session"),
    false,
  );

  phase = "relogin";
  inputs = page.locator("input");
  await inputs.nth(0).fill(username);
  await inputs.nth(1).fill(changedPassword);
  await page.getByRole("button", { name: "Login" }).click();
  await page.waitForURL(`${baseUrl}/#/`);
  await page.waitForLoadState("networkidle");
  cookies = await context.cookies(baseUrl);
  session = cookies.find((cookie) => cookie.name === "hardened-blikvm-session");
  csrf = cookies.find((cookie) => cookie.name === "hardened-blikvm-csrf");
  assert.equal(session?.secure, true);
  assert.ok(csrf);

  phase = "logout";
  const logout = await page.evaluate(async (csrfToken) => {
    const response = await fetch("/api/auth/logout", {
      method: "POST",
      credentials: "include",
      headers: { "x-csrf-token": csrfToken },
    });
    return response.status;
  }, csrf.value);
  assert.equal(logout, 200);
  cookies = await context.cookies(baseUrl);
  assert.equal(
    cookies.some((cookie) => cookie.name === "hardened-blikvm-session"),
    false,
  );

  phase = "response-audit";
  assert.equal(pageErrors.length, 0);
  assert.equal(
    responses.some((response) => response.status === 404),
    false,
  );
  for (const required of [
    ["POST", "/api/auth/setup", 200],
    ["POST", "/api/auth/login", 200],
    ["GET", "/api/inventory", 200],
    ["POST", "/api/auth/password", 200],
    ["POST", "/api/auth/logout", 200],
  ]) {
    assert.ok(
      responses.some(
        (response) =>
          response.method === required[0] &&
          response.path === required[1] &&
          response.status === required[2],
      ),
      `missing browser request ${required.join(" ")}`,
    );
  }

  phase = "complete";
  console.log(
    JSON.stringify({
      schemaVersion: 1,
      result: "passed",
      protocol: "https",
      title: "Adapter status - Hardened BliKVM",
      sessionCookie: "secure-httponly-strict",
      csrfCookie: "secure-readable-strict",
      inventoryStatus: 200,
      inventoryBackend: "kvmd_transition",
      passwordRotation: "passed",
      logout: "passed",
      apiResponsesObserved: responses.length,
    }),
  );
} catch (error) {
  const allowedTypes = new Set([
    "AssertionError",
    "Error",
    "TimeoutError",
    "TypeError",
  ]);
  const errorType = allowedTypes.has(error?.name) ? error.name : "Error";
  console.log(
    JSON.stringify({
      schemaVersion: 1,
      result: "failed",
      phase,
      errorType,
    }),
  );
  process.exitCode = 1;
} finally {
  if (browser) await browser.close();
}
