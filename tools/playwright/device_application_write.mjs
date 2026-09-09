import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { pathToFileURL } from "node:url";
import {
  activationRoute,
  uploadRoute,
  bundleBytes,
  bundleSha256,
  runApplicationWriteUi,
  validateActivationRequest,
  waitLoginInputs,
} from "./application_write_acceptance.mjs";
import { verifyStatusPanel } from "./status_api_acceptance.mjs";

const [
  mode,
  modulePath,
  executablePath,
  baseUrl,
  username,
  archive,
  outputValue,
] = process.argv.slice(2);
assert.equal(process.argv.slice(2).length, 7);
assert.ok(["activate", "closed", "preflight"].includes(mode));
const url = new URL(baseUrl);
assert.equal(url.protocol, "https:");
assert.ok(
  !url.username &&
    !url.password &&
    !url.search &&
    !url.hash &&
    url.pathname === "/",
);
assert.match(username, /^[a-z_][a-z0-9_-]{0,31}$/);
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
if (mode === "activate") {
  const bytes = fs.readFileSync(archive);
  assert.equal(bytes.length, bundleBytes);
  assert.equal(
    crypto.createHash("sha256").update(bytes).digest("hex"),
    bundleSha256,
  );
}
let password = "";
for await (const chunk of process.stdin) {
  password += chunk.toString("utf8");
  assert.ok(password.length <= 4096);
}
password = password.replace(/[\r\n]+$/, "");
assert.ok(password.length >= 8 && password.length <= 256);
const receipt = {
  probe: "application-http-write",
  mode,
  result: "failed",
  uploadRequests: 0,
  activationRequests: 0,
  loginRequests: 0,
  blockedMutations: 0,
  pageErrors: 0,
};
let browser;
let phase = "launch";
try {
  const { chromium } = await import(pathToFileURL(modulePath).href);
  browser = await chromium.launch({ executablePath, headless: true });
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    locale: "en-US",
  });
  await context.route("**/api/**", async (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;
    const method = request.method();
    if (["GET", "HEAD", "OPTIONS"].includes(method)) return route.continue();
    if (
      method === "POST" &&
      pathname === "/api/auth/login" &&
      receipt.loginRequests++ === 0
    )
      return route.continue();
    if (
      mode === "activate" &&
      method === "POST" &&
      [uploadRoute, activationRoute].includes(pathname)
    ) {
      try {
        assert.equal(new URL(request.url()).origin, url.origin);
        assert.ok(await request.headerValue("x-csrf-token"));
        if (pathname === uploadRoute) {
          assert.equal(receipt.uploadRequests++, 0);
          assert.equal(
            await request.headerValue("content-type"),
            "application/octet-stream",
          );
          const bytes = request.postDataBuffer();
          assert.equal(bytes?.length, bundleBytes);
          assert.equal(
            crypto.createHash("sha256").update(bytes).digest("hex"),
            bundleSha256,
          );
        } else {
          assert.equal(receipt.activationRequests++, 0);
          validateActivationRequest(request.postDataJSON());
        }
        return route.continue();
      } catch {
        receipt.blockedMutations++;
        return route.abort("blockedbyclient");
      }
    }
    receipt.blockedMutations++;
    await route.abort("blockedbyclient");
  });
  // No keyboard/mouse/ATX transport is needed for an update-dialog check.
  await context.routeWebSocket("**/*", (socket) => socket.close());
  const page = await context.newPage();
  page.setDefaultTimeout(15000);
  page.on("pageerror", () => receipt.pageErrors++);
  phase = "login-document";
  const documentResponse = await page.goto(`${baseUrl}/#/auth/login`, {
    waitUntil: "domcontentloaded",
  });
  receipt.documentStatus = documentResponse?.status() ?? null;
  assert.equal(documentResponse.status(), 200);
  phase = "login-title";
  await page.waitForFunction(
    () => document.title === "Login - Hardened BliKVM",
  );
  phase = "login-inputs";
  const inputs = await waitLoginInputs(page);
  receipt.loginInputCount = await inputs.count();
  assert.equal(receipt.loginInputCount, 2);
  phase = "login-fill";
  await inputs.nth(0).fill(username);
  await inputs.nth(1).fill(password);
  const response = page.waitForResponse(
    (item) =>
      new URL(item.url()).pathname === "/api/auth/login" &&
      item.request().method() === "POST",
  );
  phase = "login-submit";
  await page.getByRole("button", { name: "Login", exact: true }).click();
  assert.equal((await response).status(), 200);
  password = "";
  await page.waitForFunction(
    () => document.title === "Remote Desktop - Hardened BliKVM",
  );
  phase = mode;
  if (mode === "activate") {
    receipt.ui = await runApplicationWriteUi(page, archive);
    assert.equal(receipt.uploadRequests, 1);
    assert.equal(receipt.activationRequests, 1);
  } else {
    receipt.ui = await verifyStatusPanel(
      page,
      true,
      () => {},
      mode === "preflight" ? "seq7" : "seq8",
    );
    assert.equal(receipt.uploadRequests, 0);
    assert.equal(receipt.activationRequests, 0);
  }
  assert.equal(receipt.blockedMutations, 0);
  assert.equal(receipt.pageErrors, 0);
  receipt.result = "passed";
} catch (error) {
  // Never serialize exception messages/URLs, headers, cookies, DOM, or input.
  receipt.phase = phase;
  receipt.errorType =
    error?.name === "TimeoutError" ? "TimeoutError" : "ProbeError";
  receipt.localFrameLines = [
    ...String(error?.stack ?? "").matchAll(
      /device_application_write\.mjs:(\d+):\d+/g,
    ),
  ]
    .slice(0, 3)
    .map((match) => Number(match[1]));
} finally {
  password = "";
  await browser?.close();
  fs.writeFileSync(output, JSON.stringify(receipt, null, 2), {
    mode: 0o600,
    flag: "wx",
  });
}
if (receipt.result !== "passed") process.exitCode = 1;
