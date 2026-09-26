import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";

import { startApp } from "./app_harness.mjs";

// A release request is asynchronous; poll for it instead of assuming a fixed
// delay, which a loaded host can exceed.
async function eventually(condition, timeoutMs = 5000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (condition()) return true;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  return condition();
}

const executablePath =
  process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ??
  "/home/w0w/.cache/ms-playwright/chromium-1228/chrome-linux64/chrome";
const playwrightModule =
  process.env.PLAYWRIGHT_MODULE ??
  "/home/w0w/.local/share/playwright/node_modules/playwright/index.mjs";
const { chromium } = await import(pathToFileURL(playwrightModule).href);
const apps = [];
const app = await startApp();
apps.push(app);
const { requestDetails, baseUrl } = app;
const browser = await chromium.launch({ executablePath, headless: true });

function hidActions() {
  return requestDetails
    .filter((request) => request.route === "/api/hid/lease" && request.body)
    .map((request) => request.body.action);
}

async function openDesktop(options = {}) {
  const context = await browser.newContext(options);
  await context.addCookies([
    {
      name: "hardened-blikvm-csrf",
      value: "fixture-csrf",
      url: baseUrl,
    },
  ]);
  const page = await context.newPage();
  await page.goto(`${baseUrl}/#/`, { waitUntil: "networkidle" });
  await page.waitForFunction(
    () => document.title === "Remote Desktop - Hardened BliKVM",
  );
  return { context, page };
}

try {
  const { context, page } = await openDesktop();
  const enable = page.getByRole("button", { name: "Enable remote control" });
  await enable.waitFor({ state: "visible" });

  await page.keyboard.press("KeyA");
  await page.waitForTimeout(50);
  assert.deepEqual(
    hidActions(),
    [],
    "input before explicit enable reached HID",
  );

  await enable.click();
  const confirmation = page.getByRole("button", {
    name: "Confirm and control",
  });
  await page.getByRole("button", { name: "Cancel" }).click();
  await confirmation.waitFor({ state: "hidden" });
  await page.keyboard.press("KeyA");
  await page.waitForTimeout(50);
  assert.deepEqual(
    hidActions(),
    [],
    "input after Cancel reopened or reached HID",
  );
  assert.equal(await confirmation.isVisible(), false, "Cancel was not sticky");

  await enable.click();
  await page.getByRole("button", { name: "Confirm and control" }).click();
  await page.keyboard.press("KeyA");
  await page.waitForFunction(() =>
    performance
      .getEntriesByType("resource")
      .some((entry) => entry.name.includes("/api/hid/lease")),
  );
  await page.getByRole("button", { name: "Disable remote control" }).click();
  await page.waitForTimeout(100);
  assert.ok(hidActions().includes("acquire"));
  assert.ok(hidActions().includes("report"));
  assert.ok(hidActions().includes("release"));

  const releasesBeforeBlur = hidActions().filter(
    (action) => action === "release",
  ).length;
  await page.getByRole("button", { name: "Enable remote control" }).click();
  await page.getByRole("button", { name: "Confirm and control" }).click();
  await page.keyboard.down("KeyB");
  await page.waitForTimeout(100);
  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  assert.ok(
    await eventually(
      () =>
        hidActions().filter((action) => action === "release").length >
        releasesBeforeBlur,
    ),
    "blur did not release the active lease",
  );
  await page.keyboard.up("KeyB");

  const releasesBeforeUnmount = hidActions().filter(
    (action) => action === "release",
  ).length;
  await page.getByRole("button", { name: "Enable remote control" }).click();
  await page.getByRole("button", { name: "Confirm and control" }).click();
  await page.keyboard.press("KeyC");
  await page.waitForTimeout(100);
  await page.evaluate(() => {
    window.location.hash = "/auth/login";
  });
  await page.waitForURL(`${baseUrl}/#/auth/login`);
  assert.ok(
    await eventually(
      () =>
        hidActions().filter((action) => action === "release").length >
        releasesBeforeUnmount,
    ),
    "desktop unmount did not release the active lease",
  );
  await context.close();

  await new Promise((resolve) => setTimeout(resolve, 200));
  const mobileStart = requestDetails.length;
  const mobile = await openDesktop({
    viewport: { width: 390, height: 844 },
    isMobile: true,
    hasTouch: true,
  });
  const mobileEnable = mobile.page.getByRole("button", {
    name: "Enable remote control",
  });
  await mobileEnable.waitFor({ state: "visible" });
  await mobileEnable.click();
  await mobile.page.getByRole("button", { name: "Cancel" }).click();
  await mobile.page.waitForTimeout(50);
  assert.deepEqual(
    requestDetails
      .slice(mobileStart)
      .filter(
        (request) =>
          request.route === "/api/hid/focus" ||
          request.route === "/api/hid/lease",
      ),
    [],
    "mobile Cancel contacted the HID API",
  );
  await mobile.context.close();

  const rejectedApp = await startApp({ hidLeaseApplied: false });
  apps.push(rejectedApp);
  const rejectedContext = await browser.newContext();
  await rejectedContext.addCookies([
    {
      name: "hardened-blikvm-csrf",
      value: "fixture-csrf",
      url: rejectedApp.baseUrl,
    },
  ]);
  const rejectedPage = await rejectedContext.newPage();
  await rejectedPage.goto(`${rejectedApp.baseUrl}/#/`, {
    waitUntil: "networkidle",
  });
  await rejectedPage
    .getByRole("button", { name: "Enable remote control" })
    .click();
  await rejectedPage
    .getByRole("button", { name: "Confirm and control" })
    .click();
  await rejectedPage.keyboard.press("KeyA");
  await rejectedPage
    .getByRole("button", { name: "Enable remote control" })
    .waitFor({ state: "visible" });
  const rejectedCalls = rejectedApp.requestDetails.filter(
    (request) => request.route === "/api/hid/lease",
  ).length;
  assert.ok(rejectedCalls >= 1, "the rejected acquire was not attempted");
  await rejectedPage.keyboard.press("KeyB");
  await rejectedPage.waitForTimeout(100);
  assert.equal(
    rejectedApp.requestDetails.filter(
      (request) => request.route === "/api/hid/lease",
    ).length,
    rejectedCalls,
    "input continued after an applied=false response",
  );
  await rejectedContext.close();
} finally {
  await browser.close();
  for (const running of apps) {
    await new Promise((resolve, reject) =>
      running.server.close((error) => (error ? reject(error) : resolve())),
    );
  }
}
