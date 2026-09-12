import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { pathToFileURL } from "node:url";
import {
  watchUploadTermination,
  validateUploadTermination,
} from "./upload_termination.mjs";
import {
  uploadRoute,
  activationRoute,
  bundleBytes,
  bundleSha256,
  waitLoginInputs,
  readStatus,
  validateStaged,
} from "./application_write_acceptance.mjs";
import {
  installResponseCapture,
  readCapturedResponse,
  responseCaptureKey,
} from "./application_response_capture.mjs";

const [modulePath, executablePath, baseUrl, username, archive, outputValue] =
  process.argv.slice(2);
assert.equal(process.argv.slice(2).length, 6);
const origin = new URL(baseUrl);
assert.equal(origin.protocol, "https:");
assert.ok(
  !origin.username &&
    !origin.password &&
    !origin.search &&
    !origin.hash &&
    origin.pathname === "/",
);
assert.match(username, /^[a-z_][a-z0-9_-]{0,31}$/);
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
const digest = (bytes) =>
  crypto.createHash("sha256").update(bytes).digest("hex");
const bytes = fs.readFileSync(archive);
assert.equal(bytes.length, bundleBytes);
assert.equal(digest(bytes), bundleSha256);
const invalid = Buffer.alloc(4096, 90);
let password = "";
for await (const chunk of process.stdin) {
  password += chunk.toString("utf8");
  assert.ok(password.length <= 4096);
}
password = password.replace(/[\r\n]+$/, "");
assert.ok(password.length >= 8 && password.length <= 256);
const receipt = {
  probe: "application-upload-failures",
  result: "failed",
  uploadRequests: 0,
  activationRequests: 0,
  loginRequests: 0,
  logoutRequests: 0,
  blockedMutations: 0,
  pageErrors: 0,
  archiveSha256: bundleSha256,
  twoPagesOneSession: true,
};
let browser;
let termination;
let phase = "launch";
try {
  const { chromium } = await import(pathToFileURL(modulePath).href);
  browser = await chromium.launch({ executablePath, headless: true });
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    locale: "en-US",
    viewport: { width: 1280, height: 900 },
  });
  await installResponseCapture(context);
  await context.routeWebSocket("**/*", (socket) => socket.close());
  await context.route("**/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const method = request.method();
    try {
      assert.equal(url.origin, origin.origin);
      if (["GET", "HEAD", "OPTIONS"].includes(method)) return route.continue();
      assert.equal(method, "POST");
      if (url.pathname === "/api/auth/login") {
        assert.equal(receipt.loginRequests++, 0);
        return route.continue();
      }
      assert.ok(await request.headerValue("x-csrf-token"));
      if (url.pathname === "/api/auth/logout") {
        assert.equal(receipt.logoutRequests++, 0);
        assert.ok(["logout-partial", "await-logout-response"].includes(phase));
        return route.continue();
      }
      if (url.pathname === activationRoute) receipt.activationRequests++;
      assert.equal(url.pathname, uploadRoute);
      const index = receipt.uploadRequests++;
      assert.ok(index < 4);
      assert.equal(
        await request.headerValue("content-type"),
        "application/octet-stream",
      );
      assert.equal(
        receipt.selectionVerified,
        index === 0 ? "invalid" : "signed-seq8",
      );
      const body = request.postDataBuffer();
      if (body !== null) {
        assert.equal(body.length, index === 0 ? invalid.length : bundleBytes);
        assert.equal(
          digest(body),
          index === 0 ? digest(invalid) : bundleSha256,
        );
      }
      return route.continue();
    } catch {
      receipt.blockedMutations++;
      return route.abort("blockedbyclient");
    }
  });
  const page = await context.newPage();
  page.setDefaultTimeout(15000);
  page.on("pageerror", () => receipt.pageErrors++);
  receipt.uploadHttpStatuses = [];
  receipt.uploadTransportFailures = [];
  page.on("response", (item) => {
    if (new URL(item.url()).pathname === uploadRoute)
      receipt.uploadHttpStatuses.push(item.status());
  });
  page.on("requestfailed", (item) => {
    if (new URL(item.url()).pathname !== uploadRoute) return;
    const error = item.failure()?.errorText;
    receipt.uploadTransportFailures.push(
      [
        "net::ERR_ABORTED",
        "net::ERR_FAILED",
        "net::ERR_CONNECTION_RESET",
        "net::ERR_HTTP2_PROTOCOL_ERROR",
      ].includes(error)
        ? error
        : "other",
    );
  });
  phase = "login";
  assert.equal(
    (
      await page.goto(`${baseUrl}/#/auth/login`, {
        waitUntil: "domcontentloaded",
      })
    ).status(),
    200,
  );
  const inputs = await waitLoginInputs(page);
  await inputs.nth(0).fill(username);
  await inputs.nth(1).fill(password);
  const loggedIn = page.waitForResponse(
    (r) =>
      new URL(r.url()).pathname === "/api/auth/login" &&
      r.request().method() === "POST",
  );
  await page.getByRole("button", { name: "Login", exact: true }).click();
  assert.equal((await loggedIn).status(), 200);
  password = "";
  await page.waitForFunction(
    () => document.title === "Remote Desktop - Hardened BliKVM",
  );
  await readStatus(page, "seq8");
  const desktopUrl = page.url();
  await page.mouse.move(640, 1);
  await page.locator("svg.lucide-settings").first().click();
  const settings = page.getByRole("dialog");
  await settings.getByText("Check for Updates", { exact: true }).click();
  const panel = settings.getByTestId("bli-application-upload");
  await panel.waitFor();
  const input = panel.getByTestId("application-archive");
  async function verifySelection(expectedBytes, expectedSha, label) {
    const selected = await input.evaluate(async (element) => {
      const file = element.files[0];
      const hash = new Uint8Array(
        await crypto.subtle.digest("SHA-256", await file.arrayBuffer()),
      );
      return {
        bytes: file.size,
        sha: Array.from(hash, (byte) =>
          byte.toString(16).padStart(2, "0"),
        ).join(""),
      };
    });
    assert.deepEqual(selected, { bytes: expectedBytes, sha: expectedSha });
    receipt.selectionVerified = label;
  }
  const uploadResponse = () =>
    page.waitForResponse(
      (r) =>
        new URL(r.url()).pathname === uploadRoute &&
        r.request().method() === "POST",
      { timeout: 30000 },
    );
  async function nextObservedUpload() {
    // The existing observer intentionally rejects accidental duplicate POSTs.
    // Reset only its completed, test-owned entry between deliberate UI retries.
    await page.waitForFunction(
      ({ key, route }) => window[key]?.[route]?.finished === true,
      { key: responseCaptureKey, route: uploadRoute },
    );
    await page.evaluate(
      ({ key, route }) => {
        delete window[key][route];
      },
      { key: responseCaptureKey, route: uploadRoute },
    );
  }
  phase = "invalid-archive";
  await input.setInputFiles({
    name: "invalid.tar",
    mimeType: "application/octet-stream",
    buffer: invalid,
  });
  await verifySelection(invalid.length, digest(invalid), "invalid");
  let response = uploadResponse();
  await panel
    .getByRole("button", { name: "Upload and verify", exact: true })
    .click();
  receipt.invalidHttpStatus = (await response).status();
  assert.equal(receipt.invalidHttpStatus, 200);
  const rejected = await readCapturedResponse(page, uploadRoute, 200);
  receipt.invalidCode = rejected.code;
  assert.equal(rejected.code, -1);
  assert.equal(rejected.msg, "archive verification failed");
  assert.ok(!("data" in rejected));
  await panel
    .getByText("The archive was rejected. Check its format and signature.", {
      exact: true,
    })
    .waitFor();
  receipt.invalidArchiveRejected = true;
  phase = "cancel-partial";
  await nextObservedUpload();
  await input.setInputFiles(archive);
  await verifySelection(bundleBytes, bundleSha256, "signed-seq8");
  const cdp = await context.newCDPSession(page);
  await cdp.send("Network.enable");
  async function throttle(enabled) {
    await cdp.send("Network.emulateNetworkConditions", {
      offline: false,
      latency: 0,
      downloadThroughput: -1,
      uploadThroughput: enabled ? 65536 : -1,
    });
  }
  async function partialProgress() {
    await page.waitForFunction(() => {
      const value = Number(
        document
          .querySelector('[role="progressbar"]')
          ?.getAttribute("aria-valuenow"),
      );
      return value > 0 && value < 100;
    });
    const value = Number(
      await panel.getByRole("progressbar").getAttribute("aria-valuenow"),
    );
    assert.ok(value > 0 && value < 100);
    return value;
  }
  await throttle(true);
  await panel
    .getByRole("button", { name: "Upload and verify", exact: true })
    .click();
  receipt.cancelProgress = await partialProgress();
  await panel
    .getByRole("button", { name: "Cancel upload", exact: true })
    .click();
  await panel.getByText(/Transfer cancelled/).waitFor();
  receipt.cancelled = true;
  phase = "retry-same-archive";
  await nextObservedUpload();
  await throttle(false);
  response = uploadResponse();
  await panel
    .getByRole("button", { name: "Retry the same archive", exact: true })
    .click();
  assert.equal((await response).status(), 200);
  validateStaged(await readCapturedResponse(page, uploadRoute, 200));
  await panel
    .getByText("This verified archive is already staged", { exact: true })
    .waitFor();
  receipt.retryAlreadyPresent = true;
  await readStatus(page, "seq8");

  phase = "second-account-page";
  const account = await context.newPage();
  account.setDefaultTimeout(15000);
  account.on("pageerror", () => receipt.pageErrors++);
  await account.goto(desktopUrl, { waitUntil: "domcontentloaded" });
  await account.waitForFunction(
    () => document.title === "Remote Desktop - Hardened BliKVM",
  );
  await account.mouse.move(640, 1);
  await account.locator("svg.lucide-settings").first().click();
  await account
    .getByRole("dialog")
    .getByText("Account", { exact: true })
    .click();
  const logout = account.getByRole("button", { name: /Logout$/ });
  await logout.waitFor();
  await nextObservedUpload();
  await input.setInputFiles(archive);
  await verifySelection(bundleBytes, bundleSha256, "signed-seq8");
  await throttle(true);
  phase = "logout-partial";
  termination = watchUploadTermination(page, uploadRoute);
  await panel
    .getByRole("button", { name: "Upload and verify", exact: true })
    .click();
  receipt.logoutProgress = await partialProgress();
  const loggedOut = account.waitForResponse(
    (r) =>
      new URL(r.url()).pathname === "/api/auth/logout" &&
      r.request().method() === "POST",
  );
  await logout.click();
  termination.markLogoutRequested();
  await account.getByRole("button", { name: "Yes", exact: true }).click();
  phase = "await-logout-response";
  receipt.logoutHttpStatus = (await loggedOut).status();
  assert.equal(receipt.logoutHttpStatus, 200);
  // Partial progress was recorded before the real logout. Stop artificially
  // stretching the remaining HTTP body while awaiting its rejected result;
  // still observe this exact request ending (401 or abort), then independently
  // require both login screens and protected status HTTP 401 before success.
  await throttle(false);
  receipt.throttleReleasedAfterLogout = true;
  phase = "await-upload-revocation";
  const ended = await termination.done;
  validateUploadTermination(ended);
  receipt.uploadTermination = ended;
  termination.dispose();
  phase = "logged-out-navigation";
  await page.waitForFunction(
    () => document.title === "Login - Hardened BliKVM",
  );
  await account.waitForFunction(
    () => document.title === "Login - Hardened BliKVM",
  );
  const status = await account.evaluate(
    async () =>
      (
        await fetch("/api/application-update/status", {
          credentials: "include",
        })
      ).status,
  );
  assert.equal(status, 401);
  receipt.unauthenticatedStatus = status;
  receipt.inflightUploadRevoked = true;
  assert.equal(receipt.uploadRequests, 4);
  assert.equal(receipt.activationRequests, 0);
  assert.equal(receipt.logoutRequests, 1);
  assert.equal(receipt.blockedMutations, 0);
  assert.equal(receipt.pageErrors, 0);
  receipt.result = "passed";
} catch (error) {
  receipt.phase = phase;
  receipt.errorType =
    error?.name === "TimeoutError" ? "TimeoutError" : "ProbeError";
  receipt.localFrameLines = [
    ...String(error?.stack ?? "").matchAll(
      /device_application_upload_failures\.mjs:(\d+):\d+/g,
    ),
  ]
    .slice(0, 3)
    .map((match) => Number(match[1]));
  // No error messages, response bodies, cookies, credentials or DOM snapshots.
} finally {
  password = "";
  termination?.dispose();
  await browser?.close();
  fs.writeFileSync(output, JSON.stringify(receipt, null, 2), {
    mode: 0o600,
    flag: "wx",
  });
}
if (receipt.result !== "passed") process.exitCode = 1;
