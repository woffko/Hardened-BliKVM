// The ATX buttons when the board has forgotten the page's confirmation of the target (code audit of 2026-11-06,
// AUD-10), through the built page and a played board. The page keeps its confirmation for 15 s; the board clears
// its own when a QUIC session or a socket ends, or a lock comes, and then refuses an action with 400 "a current
// focus confirmation is required". The page confirms again and sends the action once more; a refusal that stays is
// said, and no rejection is left unhandled. Before, the press failed silently.
import assert from "node:assert/strict";
import { startApp } from "./app_harness.mjs";
import { launchChromium } from "./quality_fixture.mjs";
import { serveStillPicture } from "./take_control.mjs";

const app = await startApp();
const browser = await launchChromium();
const FOCUS_REQUIRED = "a current focus confirmation is required";

async function eventually(condition, timeoutMs = 8000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await condition()) return true;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  return condition();
}

try {
  const context = await browser.newContext({
    viewport: { width: 1280, height: 800 },
  });
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("about:blank");
  await serveStillPicture(page);

  // What the page asks, in order; the next power actions refused as the
  // board refuses them without a confirmation.
  const events = [];
  let refuse = 0;
  await page.route("**/api/hid/focus", (route) => {
    events.push(`${route.request().method()} focus`);
    return route.fallback();
  });
  await page.route("**/api/application-power", (route) => {
    events.push("power");
    if (refuse > 0) {
      refuse -= 1;
      return route.fulfill({
        status: 400,
        json: { code: -1, msg: FOCUS_REQUIRED },
      });
    }
    return route.fallback();
  });
  await page.goto(app.baseUrl, { waitUntil: "load" });
  await page.waitForFunction(
    () => document.querySelector("img#screen")?.naturalWidth > 0,
  );

  // The menu is opened by its icon and closed by it again: a key would ask
  // for remote control.
  const reset = async () => {
    await page.locator("svg.lucide-power").first().click();
    await page.getByText("Reset", { exact: true }).click();
    await page.getByRole("button", { name: "Yes" }).click();
  };
  const closeMenu = async () => {
    await page.waitForTimeout(300);
    if (await page.getByText("Reset", { exact: true }).isVisible())
      await page.locator("svg.lucide-power").first().click();
    await page.waitForTimeout(300);
  };
  const count = (event) => events.filter((one) => one === event).length;

  await reset();
  assert.ok(await eventually(() => count("power") === 1), events.join());
  await closeMenu();

  // Within the page's 15 s, the board has forgotten: refused once, the
  // press is confirmed again and sent once more.
  refuse = 1;
  const before = events.length;
  await page.waitForTimeout(300);
  await reset();
  assert.ok(
    await eventually(
      () => events.slice(before).filter((one) => one === "power").length >= 2,
    ),
    `the refused press was not sent again: ${events.slice(before).join()}`,
  );
  const again = events.slice(before);
  const refusedAt = again.indexOf("power");
  const retriedAt = again.indexOf("power", refusedAt + 1);
  assert.ok(
    again.slice(refusedAt, retriedAt).includes("GET focus") &&
      again.slice(refusedAt, retriedAt).includes("POST focus"),
    `the target was not confirmed again: ${again.join()}`,
  );
  await closeMenu();

  // A refusal that stays is said.
  refuse = 2;
  await page.waitForTimeout(300);
  await reset();
  await page
    .getByText(FOCUS_REQUIRED, { exact: false })
    .first()
    .waitFor({ state: "visible" });
  await page.waitForTimeout(300);
  assert.deepEqual(errors, [], errors.join("\n"));
  await context.close();
  console.log(
    "power focus contract: ok (a press the board refused for want of a confirmation was confirmed again and sent once more; a refusal that stayed was said, with no unhandled rejection)",
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
