// The target's disk on the toolbar (docs/baselines/work-plan-20261111-hdd-indicator.md, H3), through the built page
// and a played board: an "HDD" pill under the power icon, amber while the disk works and grey while it is idle, and no
// pill when the board has no disk LED; the Power menu shows both case LEDs. The page reads `/api/atx/leds` once a
// second while it is shown, and not while it is hidden.
import assert from "node:assert/strict";
import { startApp } from "./app_harness.mjs";
import { eventually } from "./eventually.mjs";
import { launchChromium } from "./quality_fixture.mjs";
import { serveStillPicture } from "./take_control.mjs";

const app = await startApp();
const browser = await launchChromium();

/** Says the page is hidden or shown again, as switching tabs does. */
const setVisibility = (page, state) =>
  page.evaluate((value) => {
    Object.defineProperty(document, "visibilityState", {
      value,
      configurable: true,
    });
    document.dispatchEvent(new Event("visibilitychange"));
  }, state);

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

  // What the board's case LEDs say, and how often the page asks.
  let answer = { powerLed: true, hddLed: true };
  let reads = 0;
  await page.route("**/api/atx/leds", (route) => {
    reads += 1;
    return route.fulfill({ json: { code: 0, msg: "success", data: answer } });
  });
  await page.goto(app.baseUrl, { waitUntil: "load" });

  const pill = page.locator("[data-hdd-led]");
  await pill.waitFor();
  assert.equal(await pill.getAttribute("data-hdd-led"), "active");
  assert.equal(await pill.textContent(), "HDD");

  answer = { powerLed: true, hddLed: false };
  assert.ok(
    await eventually(
      async () => (await pill.getAttribute("data-hdd-led")) === "idle",
      4000,
    ),
    "the pill did not go idle",
  );

  // The Power menu shows both LEDs; it is closed by its icon again.
  const powerIcon = page.locator("svg.lucide-power").first();
  await powerIcon.click();
  const leds = page.getByTestId("case-leds");
  await leds.waitFor();
  assert.ok(await leds.getByText("Power on", { exact: true }).isVisible());
  assert.ok(await leds.getByText("Disk", { exact: true }).isVisible());
  await powerIcon.click();

  // A board without a disk LED shows no pill.
  answer = { powerLed: true, hddLed: null };
  assert.ok(
    await eventually(async () => (await pill.count()) === 0, 4000),
    "a pill without a disk LED",
  );

  // Hidden, the page asks nothing; shown again, it asks again.
  await setVisibility(page, "hidden");
  await page.waitForTimeout(300);
  const hiddenFrom = reads;
  await page.waitForTimeout(2500);
  assert.equal(reads, hiddenFrom, "the page read while hidden");
  await setVisibility(page, "visible");
  assert.ok(
    await eventually(() => reads > hiddenFrom, 3000),
    "the page did not read again once shown",
  );
  assert.deepEqual(errors, [], errors.join("\n"));
  await context.close();
  console.log(
    "hdd indicator contract: ok (the HDD pill under the power icon followed the board, active and idle, and went away without a disk LED; the Power menu showed both LEDs; no read while hidden)",
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
