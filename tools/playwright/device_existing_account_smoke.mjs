import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [modulePath, executablePath, baseUrl, username, outputValue] =
  process.argv.slice(2);
assert.ok(
  modulePath && executablePath && baseUrl && username && outputValue,
  "missing browser arguments",
);
assert.equal(new URL(baseUrl).protocol, "https:");
assert.match(username, /^[a-z_][a-z0-9_-]{0,31}$/);

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
  });

  phase = "login-document";
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
  await page.getByRole("button", { name: "Login" }).click();
  assert.equal((await loginResponse).status(), 200);
  await page.waitForURL(`${baseUrl}/#/`);
  await page.waitForFunction(
    () => document.title === "Remote Desktop - Hardened BliKVM",
  );

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

  phase = "logout";
  await page.goto("about:blank", { waitUntil: "load" });
  const logout = await context.request.post(`${baseUrl}/api/auth/logout`, {
    headers: { Origin: baseUrl, "x-csrf-token": csrf.value },
  });
  assert.equal(logout.status(), 200);
  cookies = await context.cookies(baseUrl);
  assert.equal(
    cookies.some((cookie) => cookie.name === "hardened-blikvm-session"),
    false,
  );

  phase = "audit";
  assert.deepEqual(pageErrors, []);
  assert.equal(webSockets, 0);
  assert.equal(
    requests.some((request) => request.route.startsWith("/api/control")),
    false,
  );
  assert.equal(
    responses.some((response) => response.status >= 500),
    false,
  );
  writeReceipt({
    schemaVersion: 1,
    probe: "hardened-blikvm-existing-account-browser",
    result: "passed",
    transport: "direct-lan-https",
    title: "Remote Desktop - Hardened BliKVM",
    setupRequired: false,
    loginStatus: 200,
    inventoryStatus: inventory.status,
    diagnosticsStatus: diagnostics.status,
    secureSessionCookie: true,
    secureCsrfCookie: true,
    logoutStatus: logout.status(),
    sessionCleared: true,
    apiRequests: requests.length,
    apiResponses: responses.length,
    controlRequests: 0,
    webSockets,
    legacyBrandingAbsent: true,
    secretEmitted: false,
  });
} catch (error) {
  writeReceipt({
    schemaVersion: 1,
    probe: "hardened-blikvm-existing-account-browser",
    result: "failed",
    phase,
    errorType: error?.name || "Error",
    apiRequests: requests.length,
    apiResponses: responses.length,
    controlRequests: requests.filter((request) =>
      request.route.startsWith("/api/control"),
    ).length,
    webSockets,
    secretEmitted: false,
  });
  process.exitCode = 1;
} finally {
  password = "";
  if (browser) await browser.close();
}
