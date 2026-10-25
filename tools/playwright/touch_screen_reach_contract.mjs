// On a phone nothing may cover an edge of the remote screen. The screen is
// laid out in the part of the page left free by the menu bar at the top and
// by the extra keys, a system keyboard or the on-screen keyboard at the
// bottom, and it ends there, not beyond the page. A zoomed image scrolls
// under the cursor until its edge, with the cursor on it, is in that part:
// the target's taskbar stays within reach. When a keyboard takes room from
// the screen, the image scrolls so that a cursor in sight stays in sight.
import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
import { screenArea, startApp } from "./app_harness.mjs";

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
const PAGE_WIDTH = 390;
const PAGE_HEIGHT = 844;
const MENU_BAR_PX = 50;
const SYSTEM_KEYBOARD_PX = 300;

async function open({ systemKeyboard = false } = {}) {
  const context = await browser.newContext({
    viewport: { width: PAGE_WIDTH, height: PAGE_HEIGHT },
    isMobile: true,
    hasTouch: true,
  });
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  await context.addInitScript(() => {
    // The bar stays, and the image is zoomed beyond the room for it.
    localStorage.setItem("hardened-blikvm-menu-auto-hide", "always");
    localStorage.setItem("hardened-blikvm-video-scale", "4");
  });
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
  await page.getByTestId("menu-bar").waitFor();
  const touch = await context.newCDPSession(page);
  return { context, page, errors, touch };
}

// One finger dragged over the screen; the cursor follows it.
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

const measure = (page) =>
  page.evaluate(() => {
    const view = document.querySelector("[data-kvm-screen-scroll]");
    const image = document.getElementById("screen").getBoundingClientRect();
    const cursor = document.querySelector('[data-testid="mobile-cursor"]');
    return {
      imageTop: image.top,
      imageBottom: image.bottom,
      imageHeight: image.height,
      viewHeight: view.clientHeight,
      scrollTop: view.scrollTop,
      maxScrollTop: view.scrollHeight - view.clientHeight,
      cursorY: Number.parseFloat(cursor.style.top),
    };
  });
const near = (actual, expected) => Math.abs(actual - expected) <= 1;

// The arrow's tip is the cursor's point: where the target's cursor is, not
// the corner of the arrow's box. The corner of the arrow's outline, within
// a pixel of the point (its stroke reaches 0.6 px beyond it).
async function tipOnThePoint(page, label) {
  const tip = await page.evaluate(() => {
    const cursor = document.querySelector('[data-testid="mobile-cursor"]');
    const outline = cursor.querySelector("svg path").getBoundingClientRect();
    return {
      x: Number.parseFloat(cursor.style.left),
      y: Number.parseFloat(cursor.style.top),
      left: outline.left,
      top: outline.top,
    };
  });
  assert.ok(
    near(tip.left, tip.x) && near(tip.top, tip.y),
    `${label}: the arrow's tip at ${tip.left.toFixed(1)}, ${tip.top.toFixed(1)} for the point ${tip.x.toFixed(1)}, ${tip.y.toFixed(1)}`,
  );
}

// The screen ends where `coveredBelow` pixels of the page's bottom begin.
async function screenEndsAbove(page, coveredBelow, label) {
  await page.waitForTimeout(450); // the layout's transition
  const area = await screenArea(page);
  assert.equal(area.top, MENU_BAR_PX, `${label}: below the menu bar`);
  assert.equal(
    area.bottom,
    area.page - coveredBelow,
    `${label}: the screen ends where the cover begins`,
  );
  return area;
}

// The zoomed image's bottom edge is at the end of the screen's area.
async function bottomEdgeInSight(page, area, label) {
  const edge = await measure(page);
  assert.ok(edge.imageHeight > edge.viewHeight, `${label}: zoomed in`);
  assert.ok(near(edge.scrollTop, edge.maxScrollTop), `${label}: scrolled`);
  assert.ok(
    near(edge.imageBottom, area.bottom),
    `${label}: the image's bottom edge is in sight: ${edge.imageBottom} at ${area.bottom}`,
  );
  return edge;
}

// The screen ends above the cover, and the cursor dragged down brings the
// image's bottom edge there.
async function bottomEdgeInReach(page, touch, coveredBelow, label) {
  const area = await screenEndsAbove(page, coveredBelow, label);
  for (let stroke = 0; stroke < 4; stroke += 1)
    await drag(touch, area.top + 40, 160);
  await page.waitForTimeout(150);
  const edge = await bottomEdgeInSight(page, area, label);
  assert.ok(
    near(edge.cursorY, edge.imageBottom),
    `${label}: the cursor is on the bottom edge: ${edge.cursorY}`,
  );
  return area;
}

try {
  // The menu bar only: the bottom edge at the page's bottom, the top edge
  // below the bar.
  {
    const { context, page, errors, touch } = await open();
    const area = await bottomEdgeInReach(page, touch, 0, "menu bar");
    for (let stroke = 0; stroke < 8; stroke += 1)
      await drag(touch, area.top + 220, -160);
    await page.waitForTimeout(150);
    const edge = await measure(page);
    assert.equal(edge.scrollTop, 0);
    assert.ok(near(edge.imageTop, area.top), "the top edge below the bar");
    assert.ok(near(edge.cursorY, edge.imageTop), "the cursor on the top edge");
    await tipOnThePoint(page, "menu bar");
    assert.deepEqual(errors, []);
    await context.close();
  }

  // The extra keys (the browser lays the page out above its keyboard). The
  // cursor waits on the bottom edge: when the keys take their room, the
  // image moves up with it, before any touch.
  {
    const { context, page, errors, touch } = await open();
    await bottomEdgeInReach(page, touch, 0, "before the keys");
    await page.locator("svg.lucide-smartphone").first().locator("..").tap();
    const strip = page.getByTestId("key-helper");
    await strip.waitFor();
    const height = await strip.evaluate((element) =>
      Math.round(element.getBoundingClientRect().height),
    );
    await bottomEdgeInSight(
      page,
      await screenEndsAbove(page, height, "extra keys open"),
      "the cursor's place stays in sight",
    );
    await bottomEdgeInReach(page, touch, height, "extra keys");
    // The drag left the keys and the focus where they were.
    assert.equal(await strip.count(), 1);
    assert.deepEqual(errors, []);
    await context.close();
  }

  // The extra keys on a system keyboard that only shrinks the visual
  // viewport: the row sits on the keyboard and the screen above both.
  {
    const { context, page, errors, touch } = await open({
      systemKeyboard: true,
    });
    await page.locator("svg.lucide-smartphone").first().locator("..").tap();
    const strip = page.getByTestId("key-helper");
    await strip.waitFor();
    await page.waitForFunction(
      (px) =>
        getComputedStyle(document.querySelector('[data-testid="key-helper"]'))
          .bottom === `${px}px`,
      SYSTEM_KEYBOARD_PX,
    );
    const height = await strip.evaluate((element) =>
      Math.round(element.getBoundingClientRect().height),
    );
    await bottomEdgeInReach(
      page,
      touch,
      height + SYSTEM_KEYBOARD_PX,
      "system keyboard",
    );
    assert.deepEqual(errors, []);
    await context.close();
  }

  // The on-screen keyboard, and the whole page again once it closes.
  {
    const { context, page, errors, touch } = await open();
    await page
      .locator("svg.lucide-keyboard:not(:has(+ svg.lucide-smartphone))")
      .first()
      .tap();
    await page.getByText("On-screen keyboard", { exact: true }).tap();
    const keyboard = page.getByTestId("touch-virtual-keyboard");
    await keyboard.waitFor();
    // The keyboard is always light, on the dark page too: its keys' and its
    // language list's text is dark on their white, not the dark scheme's
    // white of a button's own text.
    const unreadable = await keyboard.evaluate((element) => {
      const channels = (colour) => colour.match(/[\d.]+/g).map(Number);
      const luminance = ([r, g, b]) => {
        const linear = (value) => {
          const share = value / 255;
          return share <= 0.04045
            ? share / 12.92
            : ((share + 0.055) / 1.055) ** 2.4;
        };
        return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
      };
      const found = [];
      for (const control of element.querySelectorAll("button, select")) {
        const style = getComputedStyle(control);
        const background = channels(style.backgroundColor);
        // A control without a background of its own (the close button).
        if (background.length > 3 && background[3] === 0) continue;
        const [lighter, darker] = [
          luminance(channels(style.color)),
          luminance(background),
        ].sort((a, b) => b - a);
        const contrast = (lighter + 0.05) / (darker + 0.05);
        if (contrast < 4.5)
          found.push(
            `${control.textContent || control.tagName}: ${style.color} on ${style.backgroundColor}`,
          );
      }
      return found;
    });
    assert.deepEqual(unreadable, [], "the keyboard's unreadable controls");
    const height = await keyboard.evaluate((element) =>
      Math.round(element.getBoundingClientRect().height),
    );
    assert.ok(height > 100);
    await bottomEdgeInReach(page, touch, height, "on-screen keyboard");
    await keyboard.locator(".lucide-x").tap();
    await keyboard.waitFor({ state: "detached" });
    await bottomEdgeInReach(page, touch, 0, "keyboard closed");
    assert.deepEqual(errors, []);
    await context.close();
  }
  console.log(
    "Touch screen reach: the screen ends above the extra keys, the system keyboard and the on-screen keyboard and below the menu bar, and a zoomed image's edges come into view under the cursor passed",
  );
} finally {
  await browser.close();
  app.server.close();
}
