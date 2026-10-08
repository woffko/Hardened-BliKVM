// How much the page enlarges the target's picture (web/src/pages/desktop/screen/use-screen-fit.ts,
// docs/design/screen-upscaling-v1.md): the fit in the view, times the zoom, times the display's pixel ratio,
// computed in one place and kept current when the window is resized, the zoom is set and the pixel ratio changes
// (the browser's zoom, another display). The page reports it on its scroll view, where probes read it.
import assert from "node:assert/strict";
import { startApp } from "./app_harness.mjs";
import { launchChromium } from "./quality_fixture.mjs";

const browser = await launchChromium();
const app = await startApp();

async function open({ width, height, ratio = 1, zoom = null }) {
  const context = await browser.newContext({
    viewport: { width, height },
    deviceScaleFactor: ratio,
  });
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  await context.addInitScript((zoom) => {
    localStorage.setItem("hardened-blikvm-menu-auto-hide", "always");
    if (zoom) localStorage.setItem("hardened-blikvm-video-scale", String(zoom));
    // A change of the pixel ratio as a browser reports it (its zoom, another
    // display): devicePixelRatio changes and a media query on the old ratio
    // stops matching. Headless Chromium's emulation changes the ratio
    // without telling the page, so the contract plays that part.
    let ratio = window.devicePixelRatio;
    const lists = [];
    Object.defineProperty(window, "devicePixelRatio", {
      get: () => ratio,
      configurable: true,
    });
    const matchMedia = window.matchMedia.bind(window);
    window.matchMedia = (query) => {
      const at = /^\(resolution: ([0-9.]+)dppx\)$/.exec(query);
      if (!at) return matchMedia(query);
      const list = new EventTarget();
      list.media = query;
      Object.defineProperty(list, "matches", {
        get: () => Number(at[1]) === ratio,
      });
      lists.push(list);
      return list;
    };
    window.__setPixelRatio = (next) => {
      ratio = next;
      for (const list of lists.splice(0)) {
        list.dispatchEvent(new Event("change"));
      }
    };
  }, zoom);
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(app.baseUrl, { waitUntil: "domcontentloaded" });
  await page.waitForSelector(
    "[data-kvm-screen-scroll][data-picture-enlargement]",
  );
  return { context, page, errors };
}

/**
 * What the page reports, and what it should: the fit of the picture (the
 * stream's 1920x1080 until it says otherwise) in the view, times the zoom,
 * times the pixel ratio.
 */
function read(page, zoom) {
  return page.evaluate((zoom) => {
    const view = document.querySelector("[data-kvm-screen-scroll]");
    const rect = view.getBoundingClientRect();
    const fit = Math.max(
      0.05,
      Math.min(rect.width / 1920, rect.height / 1080, 1),
    );
    return {
      reported: Number(view.dataset.pictureEnlargement),
      ratio: Number(view.dataset.picturePixelRatio),
      expected: fit * zoom * devicePixelRatio,
    };
  }, zoom);
}

/** Waits until the page reports what it should. */
async function settled(page, zoom) {
  let last = null;
  for (let i = 0; i < 40; i++) {
    last = await read(page, zoom);
    if (Math.abs(last.reported - last.expected) < 0.002) return last;
    await page.waitForTimeout(100);
  }
  assert.fail(`enlargement ${last.reported}, expected ${last.expected}`);
}

try {
  {
    const { context, page, errors } = await open({ width: 1280, height: 720 });
    const plain = await settled(page, 1);
    assert.ok(plain.reported < 1, "a 1920x1080 picture fits a 1280x720 page");
    assert.equal(plain.ratio, 1);

    // The browser's zoom or another display: the ratio changes under the page.
    await page.evaluate(() => window.__setPixelRatio(1.5));
    const zoomedBrowser = await settled(page, 1);
    assert.equal(zoomedBrowser.ratio, 1.5);
    await page.evaluate(() => window.__setPixelRatio(1));
    await settled(page, 1);

    // A larger window fits the picture larger.
    await page.setViewportSize({ width: 1700, height: 960 });
    // The page lays the view out again after the window's resize.
    await page.waitForFunction(
      () =>
        document
          .querySelector("[data-kvm-screen-scroll]")
          .getBoundingClientRect().width > 1600,
    );
    const larger = await settled(page, 1);
    assert.ok(
      larger.reported > plain.reported,
      `${JSON.stringify(larger)} after ${JSON.stringify(plain)}`,
    );
    assert.deepEqual(errors, []);
    await context.close();
  }
  {
    // The zoom set to 200 % on a display of ratio 2.
    const { context, page, errors } = await open({
      width: 1280,
      height: 720,
      ratio: 2,
      zoom: 2,
    });
    const both = await settled(page, 2);
    assert.equal(both.ratio, 2);
    assert.ok(both.reported > 2, String(both.reported));
    assert.deepEqual(errors, []);
    await context.close();
  }
  console.log(
    "Screen enlargement: fit x zoom x pixel ratio reported, and kept current through a pixel-ratio change, a resize and the zoom",
  );
} finally {
  await browser.close();
  app.server.close();
}
