// The target's disk on the toolbar (docs/baselines/work-plan-20261111-hdd-indicator.md, H3 and H7), through the built
// page and a played board: an "HDD" pill under the power icon, amber while the disk's LED is lit and grey while it is
// dark, and no pill when the board has no disk LED; the Power menu shows both case LEDs. As KVMD does, the page hears
// each change over a socket, `/api/atx/leds/socket`, while it is shown; hidden, it closes the socket; when the socket
// is down, it reads `/api/atx/leds` once a second and opens a new socket after 2 s.
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

  // The board's sockets, the one open now, and the light route's reads.
  const sockets = [];
  let open = null;
  await page.routeWebSocket("**/api/atx/leds/socket", (socket) => {
    const entry = { socket, closed: false };
    socket.onClose(() => {
      entry.closed = true;
      if (open === entry) open = null;
    });
    sockets.push(entry);
    open = entry;
    socket.send(JSON.stringify({ powerLed: true, hddLed: true }));
  });
  const send = (leds) => open.socket.send(JSON.stringify(leds));
  let answer = { powerLed: true, hddLed: false };
  let reads = 0;
  await page.route("**/api/atx/leds", (route) => {
    reads += 1;
    return route.fulfill({ json: { code: 0, msg: "success", data: answer } });
  });
  await page.goto(app.baseUrl, { waitUntil: "load" });

  const pill = page.locator("[data-hdd-led]");
  const state = () =>
    page.evaluate(
      () =>
        document.querySelector("[data-hdd-led]")?.getAttribute("data-hdd-led") ??
        "none",
    );
  await pill.waitFor();
  assert.equal(sockets.length, 1, "one socket");
  assert.equal(await pill.getAttribute("data-hdd-led"), "active");
  assert.equal(await pill.textContent(), "HDD");

  // Each change shows at once: the pill follows the socket within a frame or
  // two, as KVMD's page follows its LEDs.
  for (const hddLed of [false, true, false]) {
    send({ powerLed: true, hddLed });
    const want = hddLed ? "active" : "idle";
    assert.ok(
      await eventually(async () => (await state()) === want, 250),
      `the pill did not show ${want} within 250 ms`,
    );
  }
  assert.equal(reads, 0, "the light route was read while the socket was open");

  // The Power menu shows both LEDs; it is closed by its icon again.
  const powerIcon = page.locator("svg.lucide-power").first();
  await powerIcon.click();
  const leds = page.getByTestId("case-leds");
  await leds.waitFor();
  assert.ok(await leds.getByText("Power on", { exact: true }).isVisible());
  assert.ok(await leds.getByText("Disk", { exact: true }).isVisible());
  await powerIcon.click();

  // A board without a disk LED shows no pill.
  send({ powerLed: true, hddLed: null });
  assert.ok(
    await eventually(async () => (await state()) === "none", 1000),
    "a pill without a disk LED",
  );

  // Hidden, the page closes its socket and reads nothing; shown again, it
  // opens a new one.
  await setVisibility(page, "hidden");
  assert.ok(
    await eventually(() => sockets[0].closed, 2000),
    "the socket stayed open while the page was hidden",
  );
  await page.waitForTimeout(2500);
  assert.equal(sockets.length, 1, "a socket opened while the page was hidden");
  assert.equal(reads, 0, "the page read while hidden");
  await setVisibility(page, "visible");
  assert.ok(
    await eventually(() => sockets.length === 2 && open !== null, 3000),
    "no new socket once the page was shown",
  );
  assert.ok(
    await eventually(async () => (await state()) === "active", 1000),
    "the new socket's LEDs were not shown",
  );

  // The socket goes down: the page reads the light route once a second and
  // shows its answer, then opens a new socket.
  const down = open;
  down.socket.close();
  assert.ok(
    await eventually(async () => (await state()) === "idle", 1500),
    "the light route's answer was not shown with the socket down",
  );
  assert.ok(
    await eventually(() => sockets.length === 3, 4000),
    "no new socket after the old one went down",
  );
  assert.ok(reads >= 1 && reads <= 4, `${reads} reads while the socket was down`);
  const settled = reads;
  await page.waitForTimeout(1500);
  assert.equal(reads, settled, "the light route was read with a socket open");

  assert.deepEqual(errors, [], errors.join("\n"));
  await context.close();
  console.log(
    "hdd indicator contract: ok (the HDD pill under the power icon followed each change the socket sent within 250 ms, and went away without a disk LED; the Power menu showed both LEDs; hidden, the page closed its socket and read nothing; with the socket down it read the light route and opened a new socket)",
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
