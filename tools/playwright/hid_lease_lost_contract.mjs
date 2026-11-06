// A HID lease that ends under the page (code audit of 2026-11-06, AUD-05), through the built page and a played
// board: the board refuses a renewal. The page forgets that lease, so the Monitors menu, which holds the lease
// without input before it asks the target agent, takes a new one first; it used to keep the dead lease and send
// the agent's call on it, which the board refuses ("take control first").
import assert from "node:assert/strict";
import { defaultManifest, startApp } from "./app_harness.mjs";
import { launchChromium } from "./quality_fixture.mjs";
import { serveStillPicture, takeControl } from "./take_control.mjs";

const feature = (availability) => ({
  availability,
  evidence: "source_verified",
});
const manifest = defaultManifest({
  board: {
    features: {
      device_settings: feature("read_only"),
      hid_screen_area: feature("read_write"),
      target_agent: feature("read_write"),
    },
  },
});
const app = await startApp({ manifest });
const browser = await launchChromium();
const MONITOR = {
  id: "00000000000000b2",
  name: "USB Capture",
  maker: "MSI",
  product: 0x2131,
  x: 0,
  y: 0,
  width: 2560,
  height: 1440,
  refresh_millihertz: 60000,
  primary: true,
  scale: 100,
  board: true,
};

async function eventually(condition, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await condition()) return true;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  return condition();
}

try {
  const context = await browser.newContext({
    viewport: { width: 1280, height: 800 },
  });
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  await context.addInitScript(() => {
    // about:blank, where the still picture is routed first, has no storage.
    try {
      localStorage.setItem("hardened-blikvm-menu-auto-hide", "always");
    } catch {
      // The board's own page sets it.
    }
  });
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("about:blank");
  await serveStillPicture(page);

  // What the page asks, in order: the lease's actions and the agent's calls.
  const events = [];
  let refuseRenewals = true;
  await page.route("**/api/hid/lease", (route) => {
    const action = route.request().postDataJSON()?.action;
    events.push(`lease:${action}`);
    if (action === "renew" && refuseRenewals)
      return route.fulfill({
        json: {
          code: 0,
          data: { applied: false, rejection: "lease_expired" },
        },
      });
    return route.fallback();
  });
  await page.route("**/api/hid/screen-area", (route) =>
    route.fulfill({ json: { code: 0, data: { area: null } } }),
  );
  await page.route("**/api/hid/mode", (route) =>
    route.fulfill({ json: { code: 0, data: { mode: "normal" } } }),
  );
  await page.route("**/api/target-agent", (route) =>
    route.fulfill({
      json: {
        code: 0,
        data: {
          available: true,
          enabled: true,
          open: true,
          problem: null,
          agent: {
            program: "hardened-blikvm-agent",
            version: "0.1.0",
            commit: "3073fd2d1a2b3c4d",
            system: {
              product: "Windows 11 Pro",
              release: "24H2",
              build: 26100,
              revision: 2033,
              architecture: "x86_64",
              locked: false,
            },
          },
          refused: null,
          messagesIn: 4,
          messagesOut: 4,
          dropped: 0,
        },
      },
    }),
  );
  await page.route("**/api/target-agent/call", (route) => {
    const { call } = route.request().postDataJSON();
    events.push(
      `call:${typeof call === "string" ? call : Object.keys(call)[0]}`,
    );
    const answer =
      call === "monitors"
        ? { monitors: [MONITOR] }
        : call === "keyboard"
          ? {
              keyboard: {
                current: {
                  id: "04090409",
                  language: "en-US",
                  name: "English (United States) - US",
                },
                installed: [],
              },
            }
          : "done";
    return route.fulfill({
      json: { code: 0, data: { answer, roundTripUs: 3000 } },
    });
  });
  await page.goto(app.baseUrl, { waitUntil: "load" });
  await page.waitForFunction(
    () => document.querySelector("img#screen")?.naturalWidth > 0,
  );

  await takeControl(page);
  await page.keyboard.press("KeyA");
  assert.ok(
    await eventually(() => events.includes("lease:acquire"), 8000),
    events.join(),
  );
  // The harness grants 20 s, renewed after 10.
  assert.ok(
    await eventually(() => events.includes("lease:renew"), 20000),
    events.join(),
  );
  refuseRenewals = false;
  const refused = events.length;
  await page.waitForTimeout(300);

  // The Monitors menu holds the lease, then asks the agent.
  await page.locator("svg.lucide-monitor-cog").first().click();
  assert.ok(
    await eventually(
      () => events.slice(refused).includes("call:monitors"),
      8000,
    ),
    events.slice(refused).join(),
  );
  const asked = events.slice(refused);
  const firstCall = asked.indexOf("call:monitors");
  assert.ok(
    asked.slice(0, firstCall).includes("lease:acquire"),
    `the agent was asked on the refused lease: ${asked.join()}`,
  );
  assert.deepEqual(errors, [], errors.join("\n"));
  await context.close();
  console.log(
    "hid lease lost contract: ok (a refused renewal forgotten: the Monitors menu took a new lease before it asked the agent)",
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
