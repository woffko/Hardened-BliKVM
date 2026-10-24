import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";

import { startApp } from "./app_harness.mjs";
import {
  controlIsOn,
  releaseControl,
  serveStillPicture,
  takeControl,
} from "./take_control.mjs";

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
  await page.goto("about:blank");
  await serveStillPicture(page);
  await page.goto(`${baseUrl}/#/`, { waitUntil: "networkidle" });
  await page.waitForFunction(
    () => document.title === "Remote Desktop - Hardened BliKVM",
  );
  await page.waitForFunction(
    () => document.querySelector("img#screen")?.naturalWidth > 0,
  );
  return { context, page };
}

try {
  const { context, page } = await openDesktop();
  const confirm = page.getByRole("button", { name: "Confirm and control" });
  const count = (action) =>
    hidActions().filter((candidate) => candidate === action).length;
  assert.equal(
    await page.getByRole("button", { name: "Enable remote control" }).count(),
    0,
    "the toolbar still has a control button",
  );

  // A key before any confirmation asks for control and reaches nothing.
  await page.keyboard.press("KeyA");
  await confirm.waitFor({ state: "visible" });
  assert.deepEqual(hidActions(), [], "the press that asked reached HID");
  await page.getByRole("button", { name: "Cancel" }).click();
  await confirm.waitFor({ state: "hidden" });
  // A move asks nothing; the next press asks again.
  const screen = page.locator("#screen").first();
  const box = await screen.boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.move(box.x + box.width / 3, box.y + box.height / 3);
  await page.waitForTimeout(100);
  assert.equal(await confirm.isVisible(), false, "a move asked for control");
  assert.deepEqual(hidActions(), [], "input after Cancel reached HID");
  await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
  await confirm.waitFor({ state: "visible" });
  await confirm.click();
  await confirm.waitFor({ state: "hidden" });
  assert.deepEqual(hidActions(), [], "the press that asked reached HID");
  assert.equal(await controlIsOn(page), true);

  await page.keyboard.press("KeyA");
  assert.ok(
    await eventually(() => count("report") > 0),
    "a key after confirming was not sent",
  );
  assert.ok(hidActions().includes("acquire"));

  // A window without focus gives the lease back but keeps the
  // confirmation: the next key takes the lease again without asking.
  await page.keyboard.down("KeyB");
  await page.waitForTimeout(100);
  const releasesBeforeBlur = count("release");
  const acquiresBeforeBlur = count("acquire");
  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  assert.ok(
    await eventually(() => count("release") > releasesBeforeBlur),
    "blur did not release the active lease",
  );
  await page.keyboard.up("KeyB");
  assert.equal(await controlIsOn(page), true, "blur took the confirmation");
  await page.keyboard.press("KeyC");
  assert.ok(
    await eventually(() => count("acquire") > acquiresBeforeBlur),
    "the key after blur did not take the lease again",
  );
  assert.equal(
    await confirm.isVisible(),
    false,
    "the key after blur asked again",
  );

  // The Mouse menu gives control back; the next press asks again.
  const releasesBeforeMenu = count("release");
  await releaseControl(page);
  assert.ok(
    await eventually(() => count("release") > releasesBeforeMenu),
    "Release remote control did not release the lease",
  );
  const reportsAfterRelease = count("report");
  await page.keyboard.press("KeyD");
  await confirm.waitFor({ state: "visible" });
  assert.equal(
    count("report"),
    reportsAfterRelease,
    "a key after the release was sent",
  );
  await confirm.click();
  await confirm.waitFor({ state: "hidden" });

  const releasesBeforeUnmount = count("release");
  await page.keyboard.press("KeyC");
  await page.waitForTimeout(100);
  await page.evaluate(() => {
    window.location.hash = "/auth/login";
  });
  await page.waitForURL(`${baseUrl}/#/auth/login`);
  assert.ok(
    await eventually(() => count("release") > releasesBeforeUnmount),
    "desktop unmount did not release the active lease",
  );
  await context.close();

  // A phone: a tap on the screen asks; Cancel contacts nothing.
  await new Promise((resolve) => setTimeout(resolve, 200));
  const mobileStart = requestDetails.length;
  const mobile = await openDesktop({
    viewport: { width: 390, height: 844 },
    isMobile: true,
    hasTouch: true,
  });
  const mobileScreen = mobile.page.locator("#screen").first();
  await mobileScreen.waitFor({ state: "visible" });
  const mobileBox = await mobileScreen.boundingBox();
  await mobile.page.touchscreen.tap(
    mobileBox.x + mobileBox.width / 2,
    mobileBox.y + mobileBox.height / 2,
  );
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

  // A lease that ended under the page (a QUIC session that reconnected) is
  // taken again by the report that met it, which is sent again: no key is
  // lost and nothing asks.
  const lostApp = await startApp({ lostReports: 1 });
  apps.push(lostApp);
  const lostContext = await browser.newContext();
  await lostContext.addCookies([
    {
      name: "hardened-blikvm-csrf",
      value: "fixture-csrf",
      url: lostApp.baseUrl,
    },
  ]);
  const lostPage = await lostContext.newPage();
  await lostPage.goto(`${lostApp.baseUrl}/#/`, { waitUntil: "networkidle" });
  await takeControl(lostPage);
  await lostPage.keyboard.press("KeyA");
  const lostActions = () =>
    lostApp.requestDetails
      .filter((request) => request.route === "/api/hid/lease" && request.body)
      .map((request) => request.body.action);
  assert.ok(
    await eventually(
      () =>
        lostActions().filter((action) => action === "acquire").length >= 2 &&
        lostActions().filter((action) => action === "report").length >= 3,
    ),
    `the lost lease was not taken again: ${lostActions().join(",")}`,
  );
  assert.equal(
    await controlIsOn(lostPage),
    true,
    "a lost lease took the confirmation",
  );
  await lostContext.close();

  // A board that refuses the lease: control turns off with a notice, and
  // the next press asks again.
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
  await takeControl(rejectedPage);
  await rejectedPage.keyboard.press("KeyA");
  await rejectedPage
    .getByText("Remote control was refused: backend_unavailable")
    .waitFor();
  await rejectedPage.waitForFunction(
    () => document.documentElement.dataset.remoteControl === "off",
  );
  const rejectedCalls = rejectedApp.requestDetails.filter(
    (request) => request.route === "/api/hid/lease",
  ).length;
  assert.ok(rejectedCalls >= 1, "the rejected acquire was not attempted");
  await rejectedPage.keyboard.press("KeyB");
  await rejectedPage
    .getByRole("button", { name: "Confirm and control" })
    .waitFor({ state: "visible" });
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
