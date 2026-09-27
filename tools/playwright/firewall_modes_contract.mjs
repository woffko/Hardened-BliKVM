// The BliKVM firewall section renders the reference page on the reference
// contract (/api/system/firewall) once the owner runs the four modes:
// read-only, Cancel, refusal, and a restrictive change that the page
// confirms on its own, on desktop and mobile; the video menu then offers no
// WebRTC transport. Without the modes the reviewed baseline toggle remains.
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
const feature = (availability) => ({
  availability,
  evidence: "source_verified",
});
const status = (mode, pending) => ({
  config: { mode },
  effectiveMode: mode,
  moderateActive: mode === "moderate",
  restrictedActive: mode === "restricted",
  paranoidActive: mode === "paranoid",
  webrtcBlocked: mode === "restricted" || mode === "paranoid",
  paranoidAvailable: true,
  confirmationRequired: pending,
  httpsEnabled: true,
  httpsPort: 443,
  backend: {
    preferred: "nftables",
    iptables: { installed: true, detail: "/usr/sbin/iptables" },
    ip6tables: { installed: true, detail: "/usr/sbin/ip6tables" },
    nft: { installed: true, detail: "/usr/sbin/nft" },
  },
  rules: {
    ipv4: `table inet hardened-blikvm {\n  # ${mode} ipv4\n}`,
    ipv6: `table inet hardened-blikvm {\n  # ${mode} ipv6\n}`,
    nft: `table inet hardened-blikvm {\n  # ${mode} nft\n}`,
  },
  message: "",
});
try {
  for (const [writable, accepted, mobile] of [
    [false, false, false],
    [true, false, false],
    [true, true, false],
    [true, true, true],
  ]) {
    const context = await browser.newContext(
      mobile
        ? {
            viewport: { width: 390, height: 844 },
            isMobile: true,
            hasTouch: true,
          }
        : {},
    );
    await context.addCookies([
      { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
    ]);
    const page = await context.newPage();
    page.setDefaultTimeout(8000);
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const access = feature(writable ? "read_write" : "read_only");
    const unavailable = feature("unavailable");
    const manifest = defaultManifest({
      board: {
        features: {
          system_settings: feature("read_only"),
          system_firewall: access,
          system_firewall_modes: access,
        },
      },
      network: {
        status: unavailable,
        dns: unavailable,
        ipv6: unavailable,
        wol: unavailable,
      },
    });
    manifest.video = {
      ...manifest.video,
      transports: ["mjpeg", "web_rtc_janus"],
    };
    await page.route("**/api/capabilities", (route) =>
      route.fulfill({ json: { code: 0, data: manifest } }),
    );
    let state = status("moderate", false);
    const writes = [];
    await page.route("**/api/system/firewall/mode**", () =>
      assert.fail("the reference page must not use the mode API"),
    );
    await page.route("**/api/system/firewall**", (route) => {
      const request = route.request();
      if (request.method() === "GET")
        return route.fulfill({ json: { code: 0, data: state } });
      const path = new URL(request.url()).pathname;
      writes.push({ path, body: request.postDataJSON() });
      if (!accepted)
        return route.fulfill({
          json: {
            code: -409,
            msg: "the firewall change was refused: fixture_refused",
            data: null,
          },
        });
      if (path.endsWith("/confirm")) {
        state = status(state.effectiveMode, false);
      } else {
        assert.deepEqual(request.postDataJSON(), { mode: "restricted" });
        state = status("restricted", true);
      }
      return route.fulfill({ json: { code: 0, data: state } });
    });
    await page.goto(app.baseUrl, { waitUntil: "networkidle" });
    await page.locator("svg.lucide-settings").click();
    await page.locator('[data-settings-tab="system"]').click();
    await page.getByText("Moderate Firewall mode is active").waitFor();
    await page.getByText("# moderate ipv4").waitFor();
    assert.equal(writes.length, 0);
    const restricted = page.getByRole("button", { name: /^Restricted/ });
    assert.equal(await restricted.isDisabled(), !writable);
    if (writable) {
      // The section's sticky header can cover a button scrolled to the top.
      await restricted.evaluate((button) =>
        button.scrollIntoView({ block: "center" }),
      );
      await restricted.click();
      await page.getByRole("button", { name: "Cancel", exact: true }).click();
      assert.equal(writes.length, 0, "Cancel caused a write");
      // The section's sticky header can cover a button scrolled to the top.
      await restricted.evaluate((button) =>
        button.scrollIntoView({ block: "center" }),
      );
      await restricted.click();
      await page
        .getByRole("button", { name: "Enable Restricted", exact: true })
        .click();
      if (accepted) {
        await page.getByText("Restricted Firewall mode is active").waitFor();
        // The page confirms a restrictive change itself, as the reference does.
        assert.equal(writes.length, 2, JSON.stringify(writes));
        assert.deepEqual(writes[0], {
          path: "/api/system/firewall",
          body: { mode: "restricted" },
        });
        assert.equal(writes[1].path, "/api/system/firewall/confirm");
        assert.equal(state.confirmationRequired, false);
        await page.getByText("# restricted ipv4").waitFor();
        // With WebRTC blocked the video menu keeps only MJPEG selectable.
        await page.getByRole("button", { name: "Close" }).first().click();
        await page
          .locator('[data-settings-tab="system"]')
          .waitFor({ state: "hidden" });
        await page.locator("svg.lucide-monitor").first().click();
        const videoMenu = page.getByText("Video Mode", { exact: true });
        if (mobile) await videoMenu.click();
        else await videoMenu.hover();
        const vp8 = page.locator('[data-video-mode="vp8"]');
        await vp8.waitFor();
        await page.waitForFunction(
          () =>
            document
              .querySelector('[data-video-mode="vp8"]')
              ?.getAttribute("aria-disabled") === "true",
        );
        assert.equal(
          await page
            .locator('[data-video-mode="mjpeg"]')
            .getAttribute("aria-disabled"),
          "false",
        );
      } else {
        await page.getByText("fixture_refused").first().waitFor();
        await page.getByText("Moderate Firewall mode is active").waitFor();
        assert.equal(writes.length, 1);
      }
    }
    assert.deepEqual(errors, []);
    await context.close();
  }
  console.log(
    "Firewall modes: reference page on the reference contract, read-only, Cancel, refusal, automatic desktop/mobile confirmation and the MJPEG-only video menu passed",
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
