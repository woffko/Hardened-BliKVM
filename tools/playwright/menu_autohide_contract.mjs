// The menu bar's display modes on a phone: in "Auto hide" the bar leaves
// after five seconds without a touch, a tab at the top edge brings it back,
// and while it shows the screen is laid out below it instead of under it;
// in "Always visible" it stays and keeps its room. On the desktop layout
// auto hide is unchanged and the screen keeps the whole page.
import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
import { startApp } from "./app_harness.mjs";

const { chromium } = await import(
  pathToFileURL(
    process.env.PLAYWRIGHT_MODULE ??
      "/home/w0w/.local/share/playwright/node_modules/playwright/index.mjs",
  ).href
);
const browser = await chromium.launch({
  headless: true,
  executablePath:
    process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ??
    "/home/w0w/.cache/ms-playwright/chromium-1228/chrome-linux64/chrome",
});
const app = await startApp();
const MODE_KEY = "hardened-blikvm-menu-auto-hide";
const HIDE_WAIT_MS = 6500;

async function open(mode, mobile) {
  const context = await browser.newContext(
    mobile
      ? {
          viewport: { width: 390, height: 844 },
          isMobile: true,
          hasTouch: true,
        }
      : { viewport: { width: 1440, height: 900 } },
  );
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  await context.addInitScript(
    ([key, value]) => localStorage.setItem(key, value),
    [MODE_KEY, mode],
  );
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(app.baseUrl, { waitUntil: "networkidle" });
  await page.getByTestId("menu-bar").waitFor();
  return { context, page, errors };
}

const hidden = (page) =>
  page.getByTestId("menu-bar").getAttribute("data-hidden");
// The room above the screen, once its 300 ms transition has settled.
async function reserved(page) {
  const read = () =>
    page.evaluate(
      () =>
        getComputedStyle(document.getElementById("kvm-pointer-surface"))
          .paddingTop,
    );
  await page.waitForTimeout(450);
  return await read();
}

try {
  // Phone, auto hide: shown with room, gone after the timeout, back on a tap.
  {
    const { context, page, errors } = await open("auto", true);
    assert.equal(await hidden(page), "false");
    assert.equal(await reserved(page), "50px");
    await page.waitForTimeout(HIDE_WAIT_MS);
    assert.equal(await hidden(page), "true", "auto hide on a phone");
    assert.equal(await reserved(page), "0px");
    const tab = page.getByTestId("menu-reveal");
    assert.equal(await tab.count(), 1);
    await tab.tap();
    assert.equal(await hidden(page), "false");
    assert.equal(await reserved(page), "50px");
    assert.equal(await page.getByTestId("menu-reveal").count(), 0);
    // A touch on the bar restarts the countdown.
    await page.waitForTimeout(3500);
    await page.getByTestId("menu-bar").tap({ position: { x: 4, y: 18 } });
    await page.waitForTimeout(3500);
    assert.equal(await hidden(page), "false", "a touch keeps the bar");
    await page.waitForTimeout(3000);
    assert.equal(await hidden(page), "true");
    assert.deepEqual(errors, []);
    await context.close();
  }
  // Phone, always visible: stays, keeps its room, no tab.
  {
    const { context, page, errors } = await open("always", true);
    await page.waitForTimeout(HIDE_WAIT_MS);
    assert.equal(await hidden(page), "false");
    assert.equal(await reserved(page), "50px");
    assert.equal(await page.getByTestId("menu-reveal").count(), 0);
    assert.deepEqual(errors, []);
    await context.close();
  }
  // Desktop, auto hide: as before, and the screen keeps the whole page.
  {
    const { context, page, errors } = await open("auto", false);
    assert.equal(await reserved(page), "0px");
    await page.mouse.move(700, 600);
    await page.waitForTimeout(HIDE_WAIT_MS);
    assert.equal(await hidden(page), "true", "auto hide on the desktop");
    assert.equal(await reserved(page), "0px");
    assert.equal(await page.getByTestId("menu-reveal").count(), 0);
    await page.getByTestId("menu-bar").hover({ force: true });
    assert.equal(await hidden(page), "false");
    assert.deepEqual(errors, []);
    await context.close();
  }
  console.log(
    "Menu bar: phone auto hide with reveal tab and reserved room, always visible, desktop unchanged passed",
  );
} finally {
  await browser.close();
  app.server.close();
}
