// The Screen menu's quality for the WebRTC modes: offered in H.264 and VP8
// when the board lists webrtc_bitrate, it shows the encoders' bitrate, sends
// the chosen level as webrtc_kbps and moves its check mark only after the
// read-back agrees. MJPEG never shows it.
import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
import { defaultManifest, startApp } from "./app_harness.mjs";

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
const MODE_KEY = "hardened-blikvm-video-mode";
const manifest = defaultManifest();
manifest.video = {
  ...manifest.video,
  status: { availability: "read_write", evidence: "source_verified" },
  transports: ["mjpeg", "web_rtc_janus", "web_rtc_janus_h264"],
  settings: ["fps", "resolution", "frame_detect", "webrtc_bitrate"],
};

async function open(mode) {
  const context = await browser.newContext();
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  await context.addInitScript(
    ([key, value]) => localStorage.setItem(key, value),
    [MODE_KEY, mode],
  );
  const page = await context.newPage();
  page.setDefaultTimeout(6000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));

  const posts = [];
  let kbps = 2500;
  await page.route("**/api/capabilities", (route) =>
    route.fulfill({ json: { code: 0, data: manifest } }),
  );
  // MJPEG's frame detection reads its state.
  await page.route("**/api/stream/mjpeg/detect", (route) =>
    route.fulfill({ json: { code: 0, data: { enabled: true } } }),
  );
  // The WebRTC views ask whether the firewall blocks them.
  await page.route("**/api/system/firewall**", (route) =>
    route.fulfill({
      json: {
        code: 0,
        data: { effectiveMode: "baseline", webrtcBlocked: false },
      },
    }),
  );
  await page.route("**/api/vm/screen", (route) => {
    if (route.request().method() === "POST") {
      const body = route.request().postDataJSON();
      posts.push(body);
      kbps = body.value;
      return route.fulfill({
        json: { code: 0, data: { applied: true, webrtcKbps: kbps } },
      });
    }
    return route.fulfill({
      json: {
        code: 0,
        data: {
          applied: true,
          fps: 30,
          quality: 80,
          requestedResolution: "1920x1080",
          webrtcKbps: kbps,
        },
      },
    });
  });
  await page.goto(app.baseUrl, { waitUntil: "networkidle" });
  await page.locator("svg.lucide-monitor").first().click();
  return { context, page, errors, posts };
}

const checked = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll("[data-webrtc-kbps]")]
      .filter((entry) => entry.querySelector("svg"))
      .map((entry) => Number(entry.getAttribute("data-webrtc-kbps"))),
  );

try {
  for (const mode of ["h264", "vp8"]) {
    const { context, page, errors, posts } = await open(mode);
    await page.getByText("Quality", { exact: true }).hover();
    await page.locator('[data-webrtc-kbps="5000"]').waitFor();
    assert.deepEqual(await checked(page), [2500], mode);
    assert.match(
      await page.locator('[data-webrtc-kbps="8000"]').innerText(),
      /Very high\s*8 Mbit\/s/,
    );
    await page.locator('[data-webrtc-kbps="5000"]').click();
    await page.waitForFunction(() =>
      document.querySelector('[data-webrtc-kbps="5000"] svg'),
    );
    assert.deepEqual(posts, [{ type: "webrtc_kbps", value: 5000 }], mode);
    assert.deepEqual(await checked(page), [5000], mode);
    assert.deepEqual(errors, [], mode);
    await context.close();
  }
  {
    const { context, page, errors } = await open("mjpeg");
    await page.getByText("Video Mode", { exact: true }).waitFor();
    assert.equal(await page.locator("[data-webrtc-kbps]").count(), 0);
    assert.equal(
      await page.getByText("Quality", { exact: true }).count(),
      0,
      "no quality in MJPEG on this board",
    );
    assert.deepEqual(errors, []);
    await context.close();
  }
  console.log(
    "WebRTC quality: levels shown in H.264 and VP8, a chosen level sent and read back, absent in MJPEG passed",
  );
} finally {
  await browser.close();
  app.server.close();
}
