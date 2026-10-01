// The extra keys on a phone: with the mobile keyboard open a row of the keys
// a phone keyboard lacks sits above it and keeps the focus (and so the
// system keyboard) on the hidden input. A tapped modifier goes with the next
// key, from the row or typed; Fn and "more" open further keys. The screen is
// laid out above the row, above a system keyboard that only shrinks the
// visual viewport, and above the on-screen keyboard.
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
const INPUT_ID = "hardened-blikvm-mobile-local-keyboard";
const SYSTEM_KEYBOARD_PX = 300;

// Keyboard reports sent through the lease, as 8-byte arrays.
const reports = () =>
  app.requestDetails
    .filter(
      (request) =>
        request.route === "/api/hid/lease" && request.body?.action === "report",
    )
    .map((request) => request.body.report.data);

async function eventually(condition, timeoutMs = 5000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await condition()) return true;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  return condition();
}

async function open({ systemKeyboard = false } = {}) {
  const context = await browser.newContext({
    viewport: { width: 390, height: 844 },
    isMobile: true,
    hasTouch: true,
  });
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  if (systemKeyboard) {
    // A browser that keeps the layout and shrinks only the visual viewport
    // while the system keyboard shows (Safari on iOS).
    await context.addInitScript((keyboardPx) => {
      const viewport = new EventTarget();
      let covered = 0;
      Object.defineProperties(viewport, {
        height: { get: () => window.innerHeight - covered },
        offsetTop: { get: () => 0 },
        scale: { get: () => 1 },
      });
      Object.defineProperty(window, "visualViewport", { get: () => viewport });
      document.addEventListener("focusin", () => {
        covered = keyboardPx;
        viewport.dispatchEvent(new Event("resize"));
      });
    }, SYSTEM_KEYBOARD_PX);
  }
  const page = await context.newPage();
  page.setDefaultTimeout(6000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(app.baseUrl, { waitUntil: "networkidle" });
  await page.getByRole("button", { name: "Enable remote control" }).click();
  await page.getByRole("button", { name: "Confirm and control" }).click();
  return { context, page, errors };
}

const openMobileKeyboard = (page) =>
  page.locator("svg.lucide-smartphone").first().locator("..").tap();
const inputFocused = (page) =>
  page.evaluate((id) => document.activeElement?.id === id, INPUT_ID);
const key = (page, id) => page.locator(`[data-helper-key="${id}"]`);
// The room below the screen, once its 300 ms transition has settled.
async function reservedBelow(page) {
  await page.waitForTimeout(450);
  return page.evaluate(
    () =>
      getComputedStyle(document.getElementById("kvm-pointer-surface"))
        .paddingBottom,
  );
}
const helperBox = (page) =>
  page.getByTestId("key-helper").evaluate((element) => ({
    height: Math.round(element.getBoundingClientRect().height),
    bottom: getComputedStyle(element).bottom,
  }));

try {
  {
    const { context, page, errors } = await open();
    assert.equal(await page.getByTestId("key-helper").count(), 0);
    assert.equal(await reservedBelow(page), "0px");
    await openMobileKeyboard(page);
    await page.getByTestId("key-helper").waitFor();
    assert.ok(await inputFocused(page), "the hidden input has the focus");
    const helper = await helperBox(page);
    assert.equal(helper.bottom, "0px");
    assert.equal(await reservedBelow(page), `${helper.height}px`);

    // A plain key keeps the focus and is pressed alone.
    let sent = reports().length;
    await key(page, "Escape").tap();
    assert.ok(await inputFocused(page), "a key tap keeps the focus");
    assert.ok(await eventually(() => reports().length >= sent + 3));
    assert.deepEqual(reports().slice(sent, sent + 3), [
      [0, 0, 0x29, 0, 0, 0, 0, 0],
      [0, 0, 0, 0, 0, 0, 0, 0],
      [0, 0, 0, 0, 0, 0, 0, 0],
    ]);

    // Ctrl armed, then "c" typed on the system keyboard: Ctrl+C, once.
    await key(page, "ControlLeft").tap();
    assert.equal(
      await key(page, "ControlLeft").getAttribute("aria-pressed"),
      "true",
    );
    sent = reports().length;
    await page.keyboard.type("c");
    assert.ok(await eventually(() => reports().length >= sent + 2));
    assert.deepEqual(reports().slice(sent, sent + 2), [
      [0x01, 0, 0x06, 0, 0, 0, 0, 0],
      [0, 0, 0, 0, 0, 0, 0, 0],
    ]);
    assert.equal(
      await key(page, "ControlLeft").getAttribute("aria-pressed"),
      "false",
      "a modifier goes with one key",
    );

    // Shift armed, then an arrow from the row.
    await key(page, "ShiftLeft").tap();
    sent = reports().length;
    await key(page, "ArrowUp").tap();
    assert.ok(await eventually(() => reports().length >= sent + 4));
    assert.deepEqual(reports().slice(sent, sent + 2), [
      [0x02, 0, 0, 0, 0, 0, 0, 0],
      [0x02, 0, 0x52, 0, 0, 0, 0, 0],
    ]);

    // Fn opens F1-F12 and grows the room kept below the screen.
    assert.equal(await key(page, "F5").count(), 0);
    await key(page, "fn").tap();
    sent = reports().length;
    await key(page, "F5").tap();
    assert.ok(await eventually(() => reports().length >= sent + 1));
    assert.deepEqual(reports()[sent], [0, 0, 0x3e, 0, 0, 0, 0, 0]);
    const grown = await helperBox(page);
    assert.ok(grown.height > helper.height, "the F keys take a second row");
    assert.equal(await reservedBelow(page), `${grown.height}px`);

    // More: Ctrl+Alt+Del in one tap.
    await key(page, "more").tap();
    assert.equal(await key(page, "F5").count(), 0, "one panel at a time");
    sent = reports().length;
    await key(page, "ctrl-alt-del").tap();
    assert.ok(await eventually(() => reports().length >= sent + 3));
    assert.deepEqual(reports().slice(sent, sent + 3), [
      [0x01, 0, 0, 0, 0, 0, 0, 0],
      [0x05, 0, 0, 0, 0, 0, 0, 0],
      [0x05, 0, 0x4c, 0, 0, 0, 0, 0],
    ]);
    assert.ok(await inputFocused(page));

    // Hide: the row goes, the input loses the focus, the room is given back.
    await key(page, "close").tap();
    await page.getByTestId("key-helper").waitFor({ state: "detached" });
    assert.equal(await inputFocused(page), false);
    assert.equal(await reservedBelow(page), "0px");
    assert.deepEqual(errors, []);
    await context.close();
  }

  // A system keyboard that only shrinks the visual viewport: the row sits on
  // it and the screen is laid out above both.
  {
    const { context, page, errors } = await open({ systemKeyboard: true });
    await openMobileKeyboard(page);
    await page.getByTestId("key-helper").waitFor();
    await page.waitForFunction(
      (px) =>
        getComputedStyle(document.querySelector('[data-testid="key-helper"]'))
          .bottom === `${px}px`,
      SYSTEM_KEYBOARD_PX,
    );
    const helper = await helperBox(page);
    assert.equal(
      await reservedBelow(page),
      `${helper.height + SYSTEM_KEYBOARD_PX}px`,
    );
    assert.deepEqual(errors, []);
    await context.close();
  }

  // The on-screen keyboard: the screen is laid out above it.
  {
    const { context, page, errors } = await open();
    await page
      .locator("svg.lucide-keyboard:not(:has(+ svg.lucide-smartphone))")
      .first()
      .tap();
    await page.getByText("On-screen keyboard", { exact: true }).tap();
    const keyboard = page.getByTestId("touch-virtual-keyboard");
    await keyboard.waitFor();
    const height = await keyboard.evaluate((element) =>
      Math.round(element.getBoundingClientRect().height),
    );
    assert.ok(height > 100);
    assert.equal(await reservedBelow(page), `${height}px`);
    await keyboard.locator("button").first().waitFor();
    await page
      .locator('[data-testid="touch-virtual-keyboard"] .lucide-x')
      .tap();
    await keyboard.waitFor({ state: "detached" });
    assert.equal(await reservedBelow(page), "0px");
    assert.deepEqual(errors, []);
    await context.close();
  }
  console.log(
    "Mobile keys: extra keys keep the focus, modifiers go with the next key, Fn and more panels, Ctrl+Alt+Del, room kept above the row, the system keyboard and the on-screen keyboard passed",
  );
} finally {
  await browser.close();
  app.server.close();
}
