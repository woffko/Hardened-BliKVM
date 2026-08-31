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
  const requests = [];
  const responses = [];
  const pageErrors = [];
  let webSocketsObserved = 0;
  let activeStreamResponse = null;
  let streamResponses = 0;
  page.on("pageerror", (error) => pageErrors.push(error.name));
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (path.startsWith("/api/")) {
      requests.push({ method: request.method(), path });
    }
  });
  page.on("websocket", () => {
    webSocketsObserved += 1;
  });
  page.on("response", (response) => {
    const path = new URL(response.url()).pathname;
    if (path.startsWith("/api/")) {
      responses.push({
        method: response.request().method(),
        path,
        status: response.status(),
      });
    }
    if (path === "/api/stream/mjpeg") {
      activeStreamResponse = response;
      streamResponses += 1;
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
  const initialStreamResponse = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === "/api/stream/mjpeg" &&
      response.request().method() === "GET",
  );
  await page.getByRole("button", { name: "Create account" }).click();
  assert.equal((await setupResponse).status(), 200);
  assert.equal((await loginResponse).status(), 200);
  phase = "post-login";
  await page.waitForURL(`${baseUrl}/#/`);
  const initialStream = await initialStreamResponse;
  assert.equal(initialStream.status(), 200);
  assert.match(
    initialStream.headers()["content-type"] || "",
    /^multipart\/x-mixed-replace\s*;/i,
  );
  await page.waitForFunction(
    () => document.title === "Remote Desktop - Hardened BliKVM",
  );
  await page.waitForFunction(() => {
    const screen = document.querySelector("img#screen");
    return screen instanceof HTMLImageElement && screen.naturalWidth > 0;
  });

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
  assert.equal(inventory.body.data.schemaVersion, 2);
  assert.equal(inventory.body.data.backend, "kvmd_transition");
  const thermalMillidegrees =
    inventory.body.data.thermal?.temperatureMillidegreesCelsius;
  assert.equal(Number.isInteger(thermalMillidegrees), true);
  assert.ok(thermalMillidegrees >= -40_000);
  assert.ok(thermalMillidegrees <= 150_000);
  assert.doesNotMatch(
    JSON.stringify(inventory.body.data),
    /password|credential|username|serial|ssid|token|cookie|endpoint|socketPath|macAddress/i,
  );

  phase = "capabilities";
  const capabilities = await page.evaluate(async () => {
    const response = await fetch("/api/capabilities", {
      credentials: "include",
    });
    return { status: response.status, body: await response.json() };
  });
  assert.equal(capabilities.status, 200);
  assert.equal(capabilities.body.code, 0);
  assert.equal(capabilities.body.data.video.status.availability, "read_only");
  assert.equal(capabilities.body.data.hid.status.availability, "unavailable");
  assert.equal(capabilities.body.data.power.status.availability, "unavailable");
  assert.equal(capabilities.body.data.power.ledState.availability, "read_only");
  assert.equal(
    capabilities.body.data.virtualMedia.status.availability,
    "read_only",
  );
  assert.equal(
    capabilities.body.data.board.features.thermal.availability,
    "read_only",
  );
  assert.equal(
    capabilities.body.data.board.features.thermal.evidence,
    "runtime_verified",
  );

  phase = "thermal-ui";
  const thermalText = `${(thermalMillidegrees / 1000).toFixed(1)} °C`;
  await page.getByText("SoC temperature", { exact: true }).first().waitFor();
  assert.ok((await page.getByText(thermalText, { exact: true }).count()) >= 1);

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
  const reconnectedStreamResponse = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === "/api/stream/mjpeg" &&
      response.request().method() === "GET",
  );
  await page.getByRole("button", { name: "Login" }).click();
  await page.waitForURL(`${baseUrl}/#/`);
  assert.equal((await reconnectedStreamResponse).status(), 200);
  await page.waitForFunction(() => {
    const screen = document.querySelector("img#screen");
    return screen instanceof HTMLImageElement && screen.naturalWidth > 0;
  });
  cookies = await context.cookies(baseUrl);
  session = cookies.find((cookie) => cookie.name === "hardened-blikvm-session");
  csrf = cookies.find((cookie) => cookie.name === "hardened-blikvm-csrf");
  assert.equal(session?.secure, true);
  assert.ok(csrf);
  assert.ok(activeStreamResponse, "missing active MJPEG response");
  const activeStreamFinished = activeStreamResponse.finished();

  phase = "logout-request";
  const logout = await context.request.post(`${baseUrl}/api/auth/logout`, {
    headers: {
      Origin: baseUrl,
      "x-csrf-token": csrf.value,
    },
  });
  phase = "logout-status";
  assert.equal(logout.status(), 200);
  await Promise.race([
    activeStreamFinished,
    new Promise((_, reject) =>
      setTimeout(() => reject(new Error("MJPEG stream survived logout")), 5000),
    ),
  ]);
  phase = "logout-cookies";
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
  const prohibitedControlPrefixes = [
    "/api/ws",
    "/api/vm",
    "/api/hid",
    "/api/atx",
    "/api/msd",
    "/api/stream/mjpeg/detect",
  ];
  const controlRequestsObserved = requests.filter((request) =>
    prohibitedControlPrefixes.some((prefix) => request.path.startsWith(prefix)),
  ).length;
  assert.equal(controlRequestsObserved, 0);
  assert.equal(webSocketsObserved, 0);
  assert.ok(streamResponses >= 2, "MJPEG reconnect was not observed");
  for (const required of [
    ["POST", "/api/auth/setup", 200],
    ["POST", "/api/auth/login", 200],
    ["GET", "/api/inventory", 200],
    ["GET", "/api/capabilities", 200],
    ["GET", "/api/stream/mjpeg", 200],
    ["POST", "/api/auth/password", 200],
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
      title: "Remote Desktop - Hardened BliKVM",
      sessionCookie: "secure-httponly-strict",
      csrfCookie: "secure-readable-strict",
      inventoryStatus: 200,
      inventorySchema: 2,
      inventoryBackend: "kvmd_transition",
      thermalObservation: "bounded_millidegrees",
      thermalCapability: "read_only",
      thermalEvidence: "runtime_verified",
      thermalUi: "rendered",
      videoCapability: "read_only",
      hidCapability: "unavailable",
      powerCapability: "unavailable",
      powerLedCapability: "read_only",
      mediaCapability: "read_only",
      videoTransport: "mjpeg",
      videoDecoded: "passed",
      videoReconnects: streamResponses,
      streamRevocation: "passed",
      controlRequestsObserved,
      webSocketsObserved,
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
