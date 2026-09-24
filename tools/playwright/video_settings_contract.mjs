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
const manifest = defaultManifest();
manifest.video.status.availability = "read_write";
manifest.video.settings = ["fps", "resolution", "quality"];
const app = await startApp({ manifest });
try {
  const context = await browser.newContext();
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  await context.addInitScript(() => {
    localStorage.setItem("hardened-blikvm-fps", "24");
    localStorage.setItem("hardened-blikvm-quality", "1");
  });
  const page = await context.newPage();
  page.setDefaultTimeout(4000);
  const requests = [];
  let accept = false;
  let fps = 60;
  let quality = 80;
  let resolution = "1920x1080";
  await page.route("**/api/vm/screen", (route) => {
    requests.push(route.request().method());
    if (route.request().method() === "POST") {
      const body = route.request().postDataJSON();
      if (body.type === "quality") {
        assert.deepEqual(body, { type: "quality", value: 50 });
        if (accept) quality = 50;
        return route.fulfill({
          json: {
            code: 0,
            data: accept
              ? { applied: true }
              : { applied: false, rejection: "quality_refused" },
          },
        });
      }
      if (body.type === "resolution") {
        assert.deepEqual(body, { type: "resolution", value: 480 });
        if (accept) resolution = "640x480";
        return route.fulfill({
          json: {
            code: 0,
            data: accept
              ? { applied: true }
              : { applied: false, rejection: "resolution_refused" },
          },
        });
      }
      assert.deepEqual(route.request().postDataJSON(), {
        type: "fps",
        value: 30,
      });
      if (accept) fps = 30;
      return route.fulfill({
        json: {
          code: 0,
          data: accept
            ? { applied: true }
            : { applied: false, rejection: "fps_refused" },
        },
      });
    }
    return route.fulfill({
      json: {
        code: 0,
        data: {
          applied: true,
          fps,
          requestedResolution: resolution,
          quality,
          capturedFps: fps === 60 ? 29 : 20,
          queuedFps: fps === 60 ? 28 : 19,
          width: 1280,
          height: 720,
        },
      },
    });
  });
  await page.goto(app.baseUrl, { waitUntil: "networkidle" });
  await page.locator("svg.lucide-monitor").click();
  await page.getByText("FPS", { exact: true }).hover();
  const selected = page.getByText("60Hz", { exact: true }).locator("..");
  await selected.waitFor();
  assert.equal(
    await selected.locator("svg.lucide-check").count(),
    1,
    "server FPS must override saved preference",
  );
  assert.deepEqual(
    requests,
    ["GET"],
    "opening desktop or screen settings must not write video settings",
  );
  await page.getByText("30Hz", { exact: true }).click();
  await page.getByText("fps_refused", { exact: true }).waitFor();
  assert.equal(await selected.locator("svg.lucide-check").count(), 1);
  assert.equal(
    await page.evaluate(() => localStorage.getItem("hardened-blikvm-fps")),
    "24",
  );
  accept = true;
  await page.getByText("30Hz", { exact: true }).click();
  await page.getByText(/delivers 20 fps/).waitFor();
  assert.equal(
    await page
      .getByText("30Hz", { exact: true })
      .locator("..")
      .locator("svg.lucide-check")
      .count(),
    1,
  );
  assert.equal(
    await page.evaluate(() => localStorage.getItem("hardened-blikvm-fps")),
    "30",
  );
  assert.deepEqual(requests, ["GET", "POST", "POST", "GET"]);
  await page.getByText("Resolution", { exact: true }).hover();
  const requested1080 = page.getByText("1920", { exact: true }).locator("..");
  await requested1080.waitFor();
  assert.equal(
    await requested1080.locator("svg.lucide-check").count(),
    1,
    "requested resolution must not be replaced by measured dimensions",
  );
  accept = false;
  await page.getByText("640", { exact: true }).click();
  await page.getByText("resolution_refused", { exact: true }).waitFor();
  assert.equal(await requested1080.locator("svg.lucide-check").count(), 1);
  accept = true;
  await page.getByText("640", { exact: true }).click();
  await page.waitForFunction(() =>
    [...document.querySelectorAll("span")].some(
      (node) =>
        node.textContent === "640" &&
        node.parentElement?.querySelector("svg.lucide-check"),
    ),
  );
  assert.deepEqual(requests, [
    "GET",
    "POST",
    "POST",
    "GET",
    "POST",
    "POST",
    "GET",
  ]);
  await page.getByText("Quality", { exact: true }).hover();
  accept = false;
  await page.getByText("Low", { exact: true }).click();
  await page.getByText("quality_refused", { exact: true }).waitFor();
  assert.equal(
    await page.evaluate(() => localStorage.getItem("hardened-blikvm-quality")),
    "1",
  );
  accept = true;
  await page.getByText("Low", { exact: true }).click();
  await page.waitForFunction(
    () => localStorage.getItem("hardened-blikvm-quality") === "4",
  );
  assert.equal(
    await page
      .getByText("Low", { exact: true })
      .locator("..")
      .locator("svg.lucide-check")
      .count(),
    1,
  );
  await context.close();
  console.log(
    "Video settings: read-only startup, refused FPS, verified FPS save and refreshed measurements passed",
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
