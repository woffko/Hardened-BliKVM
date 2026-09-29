// A browser whose WebRTC is switched off (a VPN or privacy extension, or a
// policy, replaces RTCPeerConnection) keeps a working page: a remembered
// WebRTC mode falls back to MJPEG, the video menu keeps both WebRTC modes
// unselectable and says why, and a stub that throws on construction is
// reported in the view instead of failing the page.
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
const UNAVAILABLE = "WebRTC is switched off in this browser";
const manifest = defaultManifest();
manifest.video = {
  ...manifest.video,
  transports: ["mjpeg", "web_rtc_janus", "web_rtc_janus_h264"],
};

async function open(stub) {
  const context = await browser.newContext();
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  // Remember H.264 (WebRTC) once, then break WebRTC the way extensions do.
  await context.addInitScript(
    ([key, stubKind]) => {
      if (!sessionStorage.getItem("fixture-mode-set")) {
        sessionStorage.setItem("fixture-mode-set", "1");
        localStorage.setItem(key, "h264");
      }
      const value =
        stubKind === "undefined"
          ? undefined
          : function RTCPeerConnection() {
              throw new TypeError("WebRTC is blocked");
            };
      Object.defineProperty(window, "RTCPeerConnection", {
        value,
        configurable: true,
        writable: true,
      });
    },
    [MODE_KEY, stub],
  );
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("**/api/capabilities", (route) =>
    route.fulfill({ json: { code: 0, data: manifest } }),
  );
  await page.route("**/api/system/firewall**", (route) =>
    route.fulfill({
      json: {
        code: 0,
        data: { effectiveMode: "baseline", webrtcBlocked: false },
      },
    }),
  );
  await page.goto(app.baseUrl, { waitUntil: "networkidle" });
  return { context, page, errors };
}

try {
  // RTCPeerConnection replaced with undefined: the page falls back to MJPEG.
  {
    const { context, page, errors } = await open("undefined");
    await page.locator("img#screen").waitFor({ state: "attached" });
    assert.equal(
      await page.evaluate((key) => localStorage.getItem(key), MODE_KEY),
      "mjpeg",
    );
    await page.locator("svg.lucide-monitor").first().click();
    await page.getByText("Video Mode", { exact: true }).hover();
    for (const mode of ["h264", "vp8"]) {
      const entry = page.locator(`[data-video-mode="${mode}"]`);
      await entry.waitFor();
      assert.equal(await entry.getAttribute("aria-disabled"), "true", mode);
    }
    assert.equal(
      await page
        .locator('[data-video-mode="mjpeg"]')
        .getAttribute("aria-disabled"),
      "false",
    );
    await page.locator('[data-video-mode="h264"]').hover();
    await page.getByText(UNAVAILABLE).waitFor();
    await page.locator('[data-video-mode="vp8"]').click();
    await page.waitForTimeout(700);
    assert.equal(
      await page.evaluate((key) => localStorage.getItem(key), MODE_KEY),
      "mjpeg",
      "a WebRTC mode was selectable",
    );
    assert.deepEqual(errors, []);
    await context.close();
  }
  // A stub that throws when constructed: the view reports it.
  {
    const { context, page, errors } = await open("throwing");
    await page.getByRole("alert").getByText(UNAVAILABLE).waitFor();
    assert.equal(await page.locator("video#screen").count(), 1);
    assert.deepEqual(errors, []);
    await context.close();
  }
  console.log(
    "WebRTC switched off in the browser: MJPEG fallback, unselectable WebRTC modes with the reason, and a reported failure instead of a page error passed",
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
