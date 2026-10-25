// On a board, with a phone's viewport and the live video: the screen is laid
// out inside the page (below the menu bar, down to the page's bottom or to
// the extra keys), and a zoomed image's bottom edge comes into view under
// the cursor. Remote control is never enabled, so the page drops the mouse
// and key reports and no HID lease is taken; the probe fails if any /api/hid
// request is made but the page's read of the desktop layout kept on the
// board (GET /api/hid/screen-area), which sends nothing to the target. The
// zoom and the menu mode are set in this browser only.
// The GUI password is read from stdin; a receipt is written under
// evidence/private. No screenshot is taken.
//
// Usage: node touch_reach_device_probe.mjs BASE_URL USERNAME OUTPUT
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

import { screenArea, watchingMayAsk } from "./app_harness.mjs";

const [baseUrl, username, outputValue] = process.argv.slice(2);
assert.equal(new URL(baseUrl).protocol, "https:");
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
let password = "";
for await (const chunk of process.stdin) password += chunk.toString("utf8");
password = password.replace(/[\r\n]+$/, "");
assert.ok(password.length >= 8);

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
const PAGE_WIDTH = 390;
const context = await browser.newContext({
  ignoreHTTPSErrors: true,
  locale: "en-US",
  viewport: { width: PAGE_WIDTH, height: 844 },
  isMobile: true,
  hasTouch: true,
});
await context.addInitScript(() => {
  localStorage.setItem("hardened-blikvm-menu-auto-hide", "always");
  localStorage.setItem("hardened-blikvm-video-scale", "4");
});
const receipt = {
  schemaVersion: 1,
  probe: "touch-reach-device",
  checks: [],
  hidRequests: [],
  errors: [],
};
const page = await context.newPage();
page.setDefaultTimeout(10000);
page.on("pageerror", (error) =>
  receipt.errors.push(String(error.message).slice(0, 200)),
);
page.on("request", (request) => {
  const route = new URL(request.url()).pathname;
  if (route.startsWith("/api/hid") && !watchingMayAsk(request.method(), route))
    receipt.hidRequests.push(`${request.method()} ${route}`);
});

async function drag(touch, fromY, distance) {
  const x = PAGE_WIDTH / 2;
  const steps = Math.ceil(Math.abs(distance) / 20);
  await touch.send("Input.dispatchTouchEvent", {
    type: "touchStart",
    touchPoints: [{ x, y: fromY }],
  });
  for (let step = 1; step <= steps; step += 1) {
    await touch.send("Input.dispatchTouchEvent", {
      type: "touchMove",
      touchPoints: [{ x, y: fromY + (distance * step) / steps }],
    });
  }
  await touch.send("Input.dispatchTouchEvent", {
    type: "touchEnd",
    touchPoints: [],
  });
}

const measure = () =>
  page.evaluate(() => {
    const view = document.querySelector("[data-kvm-screen-scroll]");
    const screen = document.getElementById("screen");
    const image = screen.getBoundingClientRect();
    return {
      element: screen.tagName.toLowerCase(),
      imageBottom: Math.round(image.bottom),
      imageHeight: Math.round(image.height),
      viewHeight: view.clientHeight,
      scrollTop: Math.round(view.scrollTop),
      maxScrollTop: view.scrollHeight - view.clientHeight,
    };
  });

// The screen ends `coveredBelow` pixels above the page's bottom and the
// zoomed image's bottom edge is there.
async function check(name, coveredBelow, touch) {
  await page.waitForTimeout(600);
  const area = await screenArea(page);
  if (touch) {
    for (let stroke = 0; stroke < 4; stroke += 1)
      await drag(touch, area.top + 40, 160);
    await page.waitForTimeout(300);
  }
  const edge = await measure();
  const entry = { name, area, coveredBelow, ...edge };
  receipt.checks.push(entry);
  assert.equal(area.top, 50, `${name}: below the menu bar`);
  assert.equal(area.bottom, area.page - coveredBelow, `${name}: screen end`);
  assert.ok(edge.imageHeight > edge.viewHeight, `${name}: zoomed in`);
  assert.ok(
    Math.abs(edge.imageBottom - area.bottom) <= 1,
    `${name}: the image's bottom edge at ${edge.imageBottom}, the screen ends at ${area.bottom}`,
  );
  console.log(
    `${name}: screen ${area.top}..${area.bottom} of ${area.page}, image bottom ${edge.imageBottom}`,
  );
}

let result = "failed";
try {
  const login = await context.request.post(`${baseUrl}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: baseUrl },
  });
  password = "";
  assert.ok((await login.json()).data?.csrfToken, "login failed");
  await page.goto(`${baseUrl}/`, { waitUntil: "domcontentloaded" });
  await page.getByTestId("menu-bar").waitFor({ timeout: 30000 });
  await page.locator("#screen").waitFor({ timeout: 30000 });
  await page.waitForTimeout(4000);
  const touch = await context.newCDPSession(page);

  await check("menu bar", 0, touch);

  // The extra keys: opened without a touch on the screen, the image follows
  // the cursor waiting on the bottom edge; then the cursor is dragged again.
  await page.locator("svg.lucide-smartphone").first().locator("..").tap();
  const strip = page.getByTestId("key-helper");
  await strip.waitFor();
  const height = await strip.evaluate((element) =>
    Math.round(element.getBoundingClientRect().height),
  );
  const row = await strip.evaluate((element) => {
    const keys = element.querySelector('[data-testid="key-helper-row"]');
    return {
      scrolls: keys.scrollWidth > keys.clientWidth,
      keys: keys.querySelectorAll("[data-helper-key]").length,
    };
  });
  receipt.keyRow = { height, ...row };
  assert.ok(row.scrolls && row.keys >= 20, "one scrolling row of keys");
  await check("extra keys open", height, null);
  await check("extra keys", height, touch);
  await strip.locator('[data-helper-key="close"]').tap();
  await strip.waitFor({ state: "detached" });
  await check("keys closed", 0, touch);

  assert.deepEqual(receipt.hidRequests, [], "no HID request without control");
  assert.deepEqual(receipt.errors, []);
  result = "passed";
} catch (error) {
  receipt.errors.push(String(error?.message ?? error).slice(0, 300));
} finally {
  receipt.result = result;
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1), { mode: 0o600 });
  await browser.close();
}
console.log(`result: ${result}`);
process.exit(result === "passed" ? 0 : 1);
