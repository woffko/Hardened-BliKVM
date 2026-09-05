import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { statusRoute, verifyStatusPanel } from "./status_api_acceptance.mjs";

const [
  modulePath,
  executablePath,
  baseUrl,
  username,
  outputValue,
  runTag,
  statusMode,
] = process.argv.slice(2);
assert.ok(
  modulePath && executablePath && baseUrl && username && outputValue,
  "missing browser arguments",
);
assert.equal(new URL(baseUrl).protocol, "https:");
assert.match(username, /^[a-z_][a-z0-9_-]{0,31}$/);
assert.ok(
  statusMode === undefined || ["enabled", "restored"].includes(statusMode),
);
if (runTag)
  assert.match(
    runTag,
    /^(?:hardened-blikvm-slot-serving-[a-f0-9]{24}|hardened-blikvm-application-update-[a-f0-9]{24}-(?:rehearsal|confirmed)|hardened-blikvm-status-api-[a-f0-9]{24}-(?:enabled|restored))$/,
  );

const outputPath = path.resolve(outputValue);
const privateRoot = path.resolve("evidence/private");
assert.equal(path.dirname(outputPath), privateRoot);

let password = "";
for await (const chunk of process.stdin) {
  password += chunk.toString("utf8");
  assert.ok(password.length <= 4096, "password input exceeded limit");
}
password = password.replace(/[\r\n]+$/, "");
assert.ok(
  password.length >= 8 && password.length <= 256,
  "invalid password input",
);

function writeReceipt(value) {
  fs.mkdirSync(privateRoot, { recursive: true, mode: 0o700 });
  const temporary = `${outputPath}.part-${process.pid}`;
  fs.writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, {
    encoding: "utf8",
    flag: "wx",
    mode: 0o600,
  });
  fs.renameSync(temporary, outputPath);
  fs.chmodSync(outputPath, 0o600);
}

let browser = null;
let phase = "launch";
const requests = [];
const responses = [];
let webSockets = 0;
let streamResponses = 0;
let activeStreamResponse = null;
let inventoryVideoOnline = null;
let inventoryVideoBusy = null;
let logoutStatus = null;
let statusAcceptance = null;

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
    const route = new URL(request.url()).pathname;
    if (route.startsWith("/api/")) {
      requests.push({ method: request.method(), route });
    }
  });
  page.on("response", (response) => {
    const route = new URL(response.url()).pathname;
    if (route.startsWith("/api/")) {
      responses.push({
        method: response.request().method(),
        route,
        status: response.status(),
      });
    }
    if (route === "/api/stream/mjpeg" && response.status() === 200) {
      streamResponses += 1;
      activeStreamResponse = response;
    }
  });

  phase = "login-document";
  if (statusMode) {
    assert.equal(
      (await context.request.get(`${baseUrl}${statusRoute}`)).status(),
      401,
    );
  }
  const documentResponse = await page.goto(`${baseUrl}/#/auth/login`, {
    waitUntil: "networkidle",
  });
  assert.equal(documentResponse?.status(), 200);
  assert.equal(
    documentResponse?.headers()["x-content-type-options"],
    "nosniff",
  );
  assert.equal(documentResponse?.headers()["x-frame-options"], "DENY");
  assert.equal(documentResponse?.headers()["referrer-policy"], "no-referrer");
  await page.waitForFunction(
    () => document.title === "Login - Hardened BliKVM",
  );
  assert.equal(await page.locator("input").count(), 2);

  phase = "setup-closed";
  const setup = await page.evaluate(async () => {
    const result = await fetch("/api/auth/setup", { credentials: "include" });
    return { status: result.status, body: await result.json() };
  });
  assert.equal(setup.status, 200);
  assert.equal(setup.body.code, 0);
  assert.equal(setup.body.data.required, false);

  phase = "login";
  const inputs = page.locator("input");
  await inputs.nth(0).fill(username);
  await inputs.nth(1).fill(password);
  const loginResponse = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === "/api/auth/login" &&
      response.request().method() === "POST",
  );
  const initialStreamResponse = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === "/api/stream/mjpeg" &&
      response.request().method() === "GET",
  );
  await page.getByRole("button", { name: "Login" }).click();
  assert.equal((await loginResponse).status(), 200);
  await page.waitForURL(`${baseUrl}/#/`);
  await page.waitForFunction(
    () => document.title === "Remote Desktop - Hardened BliKVM",
  );
  phase = "inventory-before-video";
  const inventoryBeforeVideo = await page.evaluate(async () => {
    const result = await fetch("/api/inventory", { credentials: "include" });
    return { status: result.status, body: await result.json() };
  });
  assert.equal(inventoryBeforeVideo.status, 200);
  assert.equal(inventoryBeforeVideo.body.code, 0);
  assert.equal(inventoryBeforeVideo.body.data.schemaVersion, 3);
  inventoryVideoOnline = inventoryBeforeVideo.body.data.video?.online ?? null;
  inventoryVideoBusy = inventoryBeforeVideo.body.data.video?.busy ?? null;

  phase = "video-initial-response";
  const initialStream = await initialStreamResponse;
  assert.equal(initialStream.status(), 200);
  assert.match(
    initialStream.headers()["content-type"] || "",
    /^multipart\/x-mixed-replace\s*;/i,
  );
  phase = "video-initial-decode";
  await page.waitForFunction(() => {
    const screen = document.querySelector("img#screen");
    return screen instanceof HTMLImageElement && screen.naturalWidth > 0;
  });
  assert.ok(
    activeStreamResponse,
    "missing active MJPEG response before reload",
  );
  const initialStreamFinished = activeStreamResponse.finished();

  phase = "video-reconnect-response";
  const reconnectedStreamResponse = page.waitForResponse(
    (response) =>
      new URL(response.url()).pathname === "/api/stream/mjpeg" &&
      response.request().method() === "GET",
  );
  await page.reload({ waitUntil: "domcontentloaded" });
  const reconnectedStream = await reconnectedStreamResponse;
  assert.equal(reconnectedStream.status(), 200);
  assert.match(
    reconnectedStream.headers()["content-type"] || "",
    /^multipart\/x-mixed-replace\s*;/i,
  );
  phase = "video-reconnect-decode";
  await page.waitForFunction(() => {
    const screen = document.querySelector("img#screen");
    return screen instanceof HTMLImageElement && screen.naturalWidth > 0;
  });
  phase = "video-reconnect-old-stream-release";
  await Promise.race([
    initialStreamFinished,
    new Promise((_, reject) =>
      setTimeout(
        () => reject(new Error("initial MJPEG stream survived reload")),
        5000,
      ),
    ),
  ]);

  phase = "authenticated";
  const inventory = await page.evaluate(async () => {
    const result = await fetch("/api/inventory", { credentials: "include" });
    return { status: result.status, body: await result.json() };
  });
  assert.equal(inventory.status, 200);
  assert.equal(inventory.body.code, 0);
  assert.equal(inventory.body.data.schemaVersion, 3);
  assert.equal(inventory.body.data.backend, "kvmd_transition");
  assert.doesNotMatch(
    JSON.stringify(inventory.body.data),
    /password|credential|username|serial|ssid|token|cookie|endpoint|socketPath|macAddress/i,
  );
  const diagnostics = await page.evaluate(async () => {
    const result = await fetch("/api/diagnostics", { credentials: "include" });
    return { status: result.status, body: await result.json() };
  });
  assert.equal(diagnostics.status, 200);
  assert.equal(diagnostics.body.code, 0);
  assert.equal(diagnostics.body.data.schemaVersion, 1);
  if (statusMode) {
    phase = "status-api-desktop-mobile";
    statusAcceptance = await verifyStatusPanel(page, statusMode === "enabled");
  }

  phase = "cookies";
  let cookies = await context.cookies(baseUrl);
  const session = cookies.find(
    (cookie) => cookie.name === "hardened-blikvm-session",
  );
  const csrf = cookies.find((cookie) => cookie.name === "hardened-blikvm-csrf");
  assert.ok(session && csrf);
  assert.equal(session.httpOnly, true);
  assert.equal(session.secure, true);
  assert.equal(session.sameSite, "Strict");
  assert.equal(csrf.httpOnly, false);
  assert.equal(csrf.secure, true);
  assert.equal(csrf.sameSite, "Strict");

  phase = "logout-request";
  assert.ok(activeStreamResponse, "missing active MJPEG response");
  const activeStreamFinished = activeStreamResponse.finished();
  const logout = await context.request.post(`${baseUrl}/api/auth/logout`, {
    headers: { Origin: baseUrl, "x-csrf-token": csrf.value },
  });
  logoutStatus = logout.status();
  phase = "logout-status";
  assert.equal(logoutStatus, 200);
  phase = "logout-stream-revocation";
  await Promise.race([
    activeStreamFinished,
    new Promise((_, reject) =>
      setTimeout(() => reject(new Error("MJPEG stream survived logout")), 5000),
    ),
  ]);
  phase = "logout-cookies";
  if (statusMode) {
    assert.equal(
      (await context.request.get(`${baseUrl}${statusRoute}`)).status(),
      401,
    );
  }
  cookies = await context.cookies(baseUrl);
  assert.equal(
    cookies.some((cookie) => cookie.name === "hardened-blikvm-session"),
    false,
  );

  phase = "audit";
  assert.deepEqual(pageErrors, []);
  assert.equal(webSockets, 0);
  assert.ok(streamResponses >= 2, "MJPEG reconnect was not observed");
  assert.equal(
    requests.some((request) =>
      /^\/api\/(?:control|hid|atx|vm|msd|ws)(?:\/|$)/.test(request.route),
    ),
    false,
  );
  assert.equal(
    responses.some((response) => response.status >= 500),
    false,
  );
  writeReceipt({
    schemaVersion: 1,
    probe: "hardened-blikvm-existing-account-browser",
    runTag: runTag || null,
    result: "passed",
    statusAcceptance,
    transport: "direct-lan-https",
    title: "Remote Desktop - Hardened BliKVM",
    setupRequired: false,
    loginStatus: 200,
    inventoryStatus: inventory.status,
    diagnosticsStatus: diagnostics.status,
    videoStatus: initialStream.status(),
    videoDecoded: true,
    videoReconnects: streamResponses,
    streamRevoked: true,
    secureSessionCookie: true,
    secureCsrfCookie: true,
    logoutStatus: logout.status(),
    sessionCleared: true,
    apiRequests: requests.length,
    apiResponses: responses.length,
    controlRequests: 0,
    webSockets,
    videoResponses: streamResponses,
    inventoryVideoOnline,
    inventoryVideoBusy,
    legacyBrandingAbsent: true,
    secretEmitted: false,
  });
} catch (error) {
  writeReceipt({
    schemaVersion: 1,
    probe: "hardened-blikvm-existing-account-browser",
    runTag: runTag || null,
    result: "failed",
    phase,
    errorType: error?.name || "Error",
    apiRequests: requests.length,
    apiResponses: responses.length,
    controlRequests: requests.filter((request) =>
      request.route.startsWith("/api/control"),
    ).length,
    webSockets,
    videoResponses: streamResponses,
    inventoryVideoOnline,
    inventoryVideoBusy,
    logoutStatus,
    secretEmitted: false,
  });
  process.exitCode = 1;
} finally {
  password = "";
  if (browser) await browser.close();
}
