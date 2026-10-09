// How much the page enlarges the target's picture (web/src/pages/desktop/screen/use-screen-fit.ts,
// docs/design/screen-upscaling-v1.md): the fit in the view, times the zoom, times the display's pixel ratio,
// computed in one place and kept current when the window is resized, the zoom is set and the pixel ratio changes
// (the browser's zoom, another display). The page reports it on its scroll view, where probes read it. Upscaled, the
// picture may be laid out larger than its own size: as large as the view, or at the target's resolution.
import assert from "node:assert/strict";
import { defaultManifest, startApp } from "./app_harness.mjs";
import { launchChromium } from "./quality_fixture.mjs";

const browser = await launchChromium();
const app = await startApp();

const GPU =
  "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 (0x00002503) Direct3D11 vs_5_0 ps_5_0, D3D11)";

async function open({
  width,
  height,
  ratio = 1,
  zoom = null,
  upscale = null,
  sent = null,
}) {
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
  if (upscale) {
    // Upscaling on, in a browser that names a GPU for its WebGL.
    await context.addInitScript(
      ([gpu, upscale]) => {
        localStorage.setItem(
          "hardened-blikvm-upscale",
          JSON.stringify(upscale),
        );
        const getParameter = WebGLRenderingContext.prototype.getParameter;
        WebGLRenderingContext.prototype.getParameter = function (name) {
          if (name === 0x9246 || name === 0x1f01) return gpu;
          return getParameter.call(this, name);
        };
      },
      [GPU, upscale],
    );
  }
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  if (sent) {
    // Without the board's power control the page sends input over its
    // WebSocket at once, with no lease: what it would send is seen here.
    const manifest = defaultManifest();
    await page.route("**/api/capabilities", (route) =>
      route.fulfill({
        json: {
          code: 0,
          data: {
            ...manifest,
            power: {
              ...manifest.power,
              status: { ...manifest.power.status, availability: "read_only" },
            },
          },
        },
      }),
    );
    await page.routeWebSocket("**/api/ws", (socket) =>
      socket.onMessage((message) => {
        if (typeof message !== "string") sent.push([...message]);
      }),
    );
  }
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
 * times the pixel ratio. `fit`: at most its own size (`fit`), as large as
 * the view (`fill`), or a target's width.
 */
function read(page, zoom, fit = "fit") {
  return page.evaluate(
    ([zoom, fit]) => {
      const view = document.querySelector("[data-kvm-screen-scroll]");
      const rect = view.getBoundingClientRect();
      const whole = Math.min(rect.width / 1920, rect.height / 1080);
      const scale =
        fit === "fit"
          ? Math.min(whole, 1)
          : fit === "fill"
            ? whole
            : fit / 1920;
      return {
        reported: Number(view.dataset.pictureEnlargement),
        ratio: Number(view.dataset.picturePixelRatio),
        expected: Math.max(0.05, scale) * zoom * devicePixelRatio,
        screen: document.getElementById("screen")?.getBoundingClientRect()
          .width,
      };
    },
    [zoom, fit],
  );
}

/** Waits until the page reports what it should. */
async function settled(page, zoom, fit = "fit") {
  let last = null;
  for (let i = 0; i < 40; i++) {
    last = await read(page, zoom, fit);
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
  {
    // Upscaled in a view larger than the picture: Fill lays it out as large
    // as the view lets it; plainly it stays at its own size.
    const fill = {
      mode: "auto",
      sharpness: "medium",
      size: "fill",
      target: null,
    };
    const { context, page, errors } = await open({
      width: 2400,
      height: 1500,
      upscale: fill,
    });
    const filled = await settled(page, 1, "fill");
    assert.ok(filled.reported > 1.1, String(filled.reported));
    assert.ok(
      Math.abs(filled.screen - 1920 * filled.reported) < 2,
      `the screen is ${filled.screen} px wide`,
    );
    await context.close();
    const plain = await open({
      width: 2400,
      height: 1500,
      upscale: { ...fill, mode: "off" },
    });
    assert.equal((await settled(plain.page, 1)).reported, 1);
    await plain.context.close();
    assert.deepEqual(errors, []);
  }
  {
    // The target's resolution: a 2560x1440 target shows the picture 2560 px
    // wide, at 133 %, whatever the view.
    const { context, page, errors } = await open({
      width: 1280,
      height: 720,
      upscale: {
        mode: "auto",
        sharpness: "medium",
        size: "target",
        target: { width: 2560, height: 1440 },
      },
    });
    const target = await settled(page, 1, 2560);
    assert.ok(
      Math.abs(target.reported - 4 / 3) < 0.002,
      String(target.reported),
    );
    assert.ok(
      Math.abs(target.screen - 2560) < 2,
      `the screen is ${target.screen} px wide`,
    );
    assert.deepEqual(errors, []);
    await context.close();
  }
  {
    // The Screen menu sets the size: the target's resolution typed and
    // applied lays the picture out at it, and is kept by the browser; Fill
    // goes back. The keys typed into the menu's fields are not the target's:
    // none is sent on, as keys pressed over the picture are.
    const sent = [];
    const { context, page, errors } = await open({
      width: 1280,
      height: 720,
      upscale: {
        mode: "auto",
        sharpness: "medium",
        size: "fill",
        target: null,
      },
      sent,
    });
    await settled(page, 1, "fill");
    await page.locator("#screen").click();
    await page.keyboard.press("KeyA");
    // Keyboard reports with a key down: the capture also sends one with
    // none whenever it lets go of the keyboard.
    const keys = () =>
      sent.filter(
        (data) => data[0] === 1 && data.slice(1).some((byte) => byte !== 0),
      ).length;
    for (let attempt = 0; attempt < 40 && keys() === 0; attempt += 1) {
      await page.waitForTimeout(50);
    }
    assert.ok(keys() > 0, "a key over the picture was not sent");
    const before = keys();
    await page.locator("svg.lucide-monitor").first().click();
    await page.getByText("Upscaling", { exact: true }).hover();
    const width = page.locator("[data-upscale-target-width]");
    await width.fill("");
    await width.pressSequentially("2560");
    const height = page.locator("[data-upscale-target-height]");
    await height.fill("");
    await height.pressSequentially("1440");
    await height.press("Enter");
    const target = await settled(page, 1, 2560);
    assert.ok(Math.abs(target.screen - 2560) < 2, String(target.screen));
    assert.equal(keys(), before, "keys typed into the menu were sent on");
    assert.deepEqual(
      await page.evaluate(() =>
        JSON.parse(localStorage.getItem("hardened-blikvm-upscale")),
      ),
      {
        mode: "auto",
        sharpness: "medium",
        size: "target",
        target: { width: 2560, height: 1440 },
      },
    );
    await page.getByText("Upscaling", { exact: true }).hover();
    await page.locator('[data-upscale-entry="fill"]').click();
    await settled(page, 1, "fill");
    assert.deepEqual(errors, []);
    await context.close();
  }
  console.log(
    "Screen enlargement: fit x zoom x pixel ratio reported, and kept current through a pixel-ratio change, a resize and the zoom; upscaled, filling the view and at the target's resolution, set in the Screen menu with no key sent on",
  );
} finally {
  await browser.close();
  app.server.close();
}
