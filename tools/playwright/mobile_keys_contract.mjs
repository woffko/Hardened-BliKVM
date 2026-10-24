// The extra keys on a phone: with the mobile keyboard open, the keys a phone
// keyboard lacks sit above it as one row that scrolls sideways (the layout
// of the RustAdmin mobile client's key row) and keep the focus, and so the
// system keyboard, on the hidden input. A modifier is held on the target:
// a tap presses it until the next key, from the row or typed; a double tap
// keeps it pressed; a tap on a pressed one releases it. Fn and "more" open
// further keys in place.
import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
import { startApp } from "./app_harness.mjs";
import { takeControl } from "./take_control.mjs";

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
// A release waits this long for a second tap (lib/sticky-modifiers.ts).
const DOUBLE_TAP_MS = 300;
const CTRL = 0x01;
const SHIFT = 0x02;
const ALT = 0x04;
const WIN = 0x08;
const report = (modifiers, usage = 0) => [modifiers, 0, usage, 0, 0, 0, 0, 0];

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

// The reports an action sends: exactly these, and no more shortly after.
async function sends(action, expected, message) {
  const before = reports().length;
  await action();
  await eventually(() => reports().length >= before + expected.length);
  await new Promise((resolve) => setTimeout(resolve, 150));
  assert.deepEqual(reports().slice(before), expected, message);
}

async function open(stored = {}) {
  const context = await browser.newContext({
    viewport: { width: 390, height: 844 },
    isMobile: true,
    hasTouch: true,
  });
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  await context.addInitScript((values) => {
    for (const [key, value] of Object.entries(values))
      localStorage.setItem(key, value);
  }, stored);
  const page = await context.newPage();
  page.setDefaultTimeout(6000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(app.baseUrl, { waitUntil: "networkidle" });
  await takeControl(page);
  await page.locator("svg.lucide-smartphone").first().locator("..").tap();
  await page.getByTestId("key-helper").waitFor();
  return { context, page, errors };
}

const inputFocused = (page) =>
  page.evaluate((id) => document.activeElement?.id === id, INPUT_ID);
const key = (page, id) => page.locator(`[data-helper-key="${id}"]`);
const pressed = (page, id) => key(page, id).getAttribute("aria-pressed");
const locked = (page, id) => key(page, id).getAttribute("data-locked");
// Two taps well within the double-tap time, whatever the host's load.
const doubleTap = (page, id) =>
  key(page, id).evaluate((entry) => {
    entry.click();
    entry.click();
  });
const keyIds = (page) =>
  page
    .locator("[data-helper-key]")
    .evaluateAll((keys) => keys.map((entry) => entry.dataset.helperKey));

try {
  {
    const { context, page, errors } = await open();
    assert.ok(await inputFocused(page), "the hidden input has the focus");

    // One row that scrolls sideways, with a mark at the edge that has more.
    const layout = await page.getByTestId("key-helper").evaluate((strip) => {
      const row = strip.querySelector('[data-testid="key-helper-row"]');
      const tops = new Set(
        [...row.querySelectorAll("[data-helper-key]")].map((entry) =>
          Math.round(entry.getBoundingClientRect().top),
        ),
      );
      return {
        height: Math.round(strip.getBoundingClientRect().height),
        bottom: Math.round(strip.getBoundingClientRect().bottom),
        rows: tops.size,
        scrolls: row.scrollWidth > row.clientWidth + 100,
        groups: [...row.querySelectorAll("[data-helper-group]")].map(
          (group) => group.dataset.helperGroup,
        ),
        moreRight: strip.querySelectorAll(".lucide-chevron-right").length,
        moreLeft: strip.querySelectorAll(".lucide-chevron-left").length,
      };
    });
    assert.equal(layout.rows, 1, "all keys in one row");
    assert.ok(layout.scrolls, "the row scrolls sideways");
    assert.ok(layout.height <= 56, `a low row: ${layout.height}px`);
    assert.equal(layout.bottom, 844, "docked at the bottom of the page");
    assert.deepEqual(layout.groups, [
      "modifiers",
      "editing",
      "enter",
      "arrows",
      "navigation",
      "function-keys",
      "pause-break",
      "other",
      "close",
    ]);
    assert.deepEqual([layout.moreLeft, layout.moreRight], [0, 1]);
    assert.deepEqual(await keyIds(page), [
      "ShiftLeft",
      "ControlLeft",
      "AltLeft",
      "MetaLeft",
      "Delete",
      "Escape",
      "Tab",
      "Insert",
      "Enter",
      "ArrowLeft",
      "ArrowUp",
      "ArrowDown",
      "ArrowRight",
      "Home",
      "End",
      "PageUp",
      "PageDown",
      "fn",
      "Pause",
      "Break",
      "more",
      "PrintScreen",
      "ScrollLock",
      "ContextMenu",
      "Ctrl+C",
      "Ctrl+V",
      "Ctrl+S",
      "Ctrl+Alt+Del",
      "close",
    ]);

    // A plain key keeps the focus and is pressed alone; so does a touch
    // between the keys.
    await sends(
      () => key(page, "Escape").tap(),
      [report(0, 0x29), report(0)],
      "Esc",
    );
    assert.ok(await inputFocused(page), "a key tap keeps the focus");
    const gap = await page
      .locator('[data-helper-group="modifiers"]')
      .evaluate((group) => {
        const box = group.getBoundingClientRect();
        return { x: box.right + 10, y: box.top + box.height / 2 };
      });
    await page.touchscreen.tap(gap.x, gap.y);
    assert.ok(await inputFocused(page), "a touch on the row keeps the focus");

    // A tap presses Ctrl on the target; "c" typed on the system keyboard is
    // Ctrl+C, and Ctrl is released with it.
    await sends(() => key(page, "ControlLeft").tap(), [report(CTRL)], "Ctrl");
    assert.equal(await pressed(page, "ControlLeft"), "true");
    assert.equal(await locked(page, "ControlLeft"), null);
    await page.waitForTimeout(DOUBLE_TAP_MS + 100);
    await sends(
      () => page.keyboard.type("c"),
      [report(CTRL, 0x06), report(0)],
      "Ctrl+C typed",
    );
    assert.equal(await pressed(page, "ControlLeft"), "false");

    // A double tap keeps Shift pressed: it goes with every key until a tap
    // releases it, after the wait for a second tap.
    await sends(
      () => doubleTap(page, "ShiftLeft"),
      [report(SHIFT)],
      "Shift is pressed once by a double tap",
    );
    assert.equal(await locked(page, "ShiftLeft"), "true");
    await sends(
      () => key(page, "ArrowUp").tap(),
      [report(SHIFT, 0x52), report(SHIFT)],
      "Shift+Up",
    );
    await sends(
      () => key(page, "ArrowUp").tap(),
      [report(SHIFT, 0x52), report(SHIFT)],
      "Shift stays pressed",
    );
    assert.equal(await locked(page, "ShiftLeft"), "true");
    await page.waitForTimeout(DOUBLE_TAP_MS + 100);
    await sends(() => key(page, "ShiftLeft").tap(), [report(0)], "released");
    assert.equal(await pressed(page, "ShiftLeft"), "false");

    // A pressed modifier is released by a tap: Win alone, down and up.
    await sends(() => key(page, "MetaLeft").tap(), [report(WIN)], "Win down");
    await page.waitForTimeout(DOUBLE_TAP_MS + 100);
    await sends(() => key(page, "MetaLeft").tap(), [report(0)], "Win up");

    // A shortcut presses its own modifiers before its key.
    await sends(
      () => key(page, "Ctrl+Alt+Del").tap(),
      [report(CTRL | ALT), report(CTRL | ALT, 0x4c), report(0)],
      "Ctrl+Alt+Del",
    );
    await sends(
      () => key(page, "Break").tap(),
      [report(CTRL), report(CTRL, 0x48), report(0)],
      "Break is Ctrl+Pause",
    );
    assert.ok(await inputFocused(page));

    // Fn opens F1-F12 in the row and brings them into view.
    assert.equal(await key(page, "F5").count(), 0);
    await key(page, "fn").tap();
    await key(page, "F5").waitFor();
    assert.ok(
      await eventually(() =>
        key(page, "fn").evaluate(
          (entry) => Math.round(entry.getBoundingClientRect().left) === 6,
        ),
      ),
      "Fn moves to the start of the row",
    );
    await sends(
      () => key(page, "F5").tap(),
      [report(0, 0x3e), report(0)],
      "F5",
    );
    await key(page, "fn").tap();
    await key(page, "F5").waitFor({ state: "detached" });

    // "More" folds the groups away; the modifiers, Fn and "more" stay.
    await key(page, "more").tap();
    assert.deepEqual(await keyIds(page), [
      "ShiftLeft",
      "ControlLeft",
      "AltLeft",
      "MetaLeft",
      "fn",
      "more",
      "close",
    ]);
    await key(page, "more").tap();
    await key(page, "Escape").waitFor();

    // Hiding the keyboard releases what is still pressed.
    await doubleTap(page, "AltLeft");
    assert.equal(await locked(page, "AltLeft"), "true");
    const beforeClose = reports().length;
    await key(page, "close").tap();
    await page.getByTestId("key-helper").waitFor({ state: "detached" });
    await eventually(() => reports().length > beforeClose);
    await page.waitForTimeout(150);
    // The physical keyboard's handler, enabled again, releases its keys too.
    const closing = reports().slice(beforeClose);
    assert.ok(
      closing.length >= 1 &&
        closing.every((entry) => entry.every((byte) => byte === 0)),
      `Alt is released when the keyboard goes: ${JSON.stringify(closing)}`,
    );
    assert.equal(await inputFocused(page), false);
    assert.deepEqual(errors, []);
    await context.close();
  }

  // A Mac target (chosen in the on-screen keyboard): Cmd shortcuts and none
  // of the keys a Mac keyboard lacks.
  {
    const { context, page, errors } = await open({
      "hardened-blikvm-keyboard-system": "mac",
    });
    const ids = await keyIds(page);
    assert.ok(ids.includes("Cmd+C") && !ids.includes("Ctrl+C"));
    for (const absent of ["Pause", "Break", "PrintScreen", "Ctrl+Alt+Del"])
      assert.ok(!ids.includes(absent), absent);
    assert.equal(
      await key(page, "MetaLeft").getAttribute("aria-label"),
      "Command",
    );
    await sends(
      () => key(page, "Cmd+C").tap(),
      [report(WIN), report(WIN, 0x06), report(0)],
      "Cmd+C",
    );
    assert.deepEqual(errors, []);
    await context.close();
  }
  console.log(
    "Mobile keys: one scrolling row that keeps the focus, modifiers held for the next key or locked by a double tap, shortcuts, Fn and more, Mac keys passed",
  );
} finally {
  await browser.close();
  app.server.close();
}
