// Device test of the firewall page (docs/design/firewall-reference-contract.md):
// for each mode it opens Settings > System > Firewall, chooses the mode,
// accepts the dialog and waits for the page to report it; the page confirms a
// restrictive mode itself. Each step records the page's firewall writes, the
// owner's mode and pending state, and, in a restrictive mode, which transports
// the video menu leaves selectable. `latency` measures the owner's shared
// state (the mouse-jiggler read shares its lock with HID) while idle and while
// the firewall status is read afresh. The GUI password is read from stdin; a
// receipt is written under evidence/private. No screenshot is taken.
//
// Usage: node firewall_page_device_test.mjs BASE_URL USERNAME OUTPUT VIEWPORT STEP...
//   VIEWPORT: desktop | mobile; STEP: baseline | moderate | restricted | paranoid | latency
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [baseUrl, username, outputValue, viewport, ...steps] =
  process.argv.slice(2);
assert.equal(new URL(baseUrl).protocol, "https:");
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
assert.ok(["desktop", "mobile"].includes(viewport));
const modes = ["baseline", "moderate", "restricted", "paranoid"];
assert.ok(
  steps.length > 0 &&
    steps.every((step) => step === "latency" || modes.includes(step)),
);
let password = "";
for await (const chunk of process.stdin) password += chunk.toString("utf8");
password = password.replace(/[\r\n]+$/, "");
assert.ok(password.length >= 8);

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
const mobile = viewport === "mobile";
const context = await browser.newContext({
  ignoreHTTPSErrors: true,
  locale: "en-US",
  ...(mobile
    ? { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true }
    : { viewport: { width: 1440, height: 1000 } }),
});
const receipt = {
  schemaVersion: 1,
  probe: "firewall-page-device",
  viewport,
  steps: [],
};
function record(step, value) {
  receipt.steps.push({ step, at: new Date().toISOString(), ...value });
  console.log(JSON.stringify({ step, ...value }).slice(0, 400));
}
const title = (mode) => mode[0].toUpperCase() + mode.slice(1);
const applyLabel = {
  baseline: "Switch to Baseline",
  moderate: "Switch to Moderate",
  restricted: "Enable Restricted",
  paranoid: "Enable Paranoid",
};

let result = "failed";
try {
  const login = await context.request.post(`${baseUrl}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: baseUrl },
  });
  password = "";
  const csrf = (await login.json()).data?.csrfToken;
  assert.ok(csrf, "login failed");
  const api = async (route) => {
    const started = Date.now();
    const response = await context.request.get(`${baseUrl}${route}`, {
      headers: { Origin: baseUrl, "X-CSRF-Token": csrf },
      timeout: 60000,
    });
    return { ms: Date.now() - started, body: await response.json() };
  };
  const page = await context.newPage();
  const writes = [];
  page.on("response", (response) => {
    const request = response.request();
    if (
      request.method() === "POST" &&
      response.url().includes("/api/system/firewall")
    )
      writes.push({
        path: new URL(response.url()).pathname,
        http: response.status(),
      });
  });
  await page.goto(`${baseUrl}/`, { waitUntil: "domcontentloaded" });

  async function openFirewall() {
    await page.locator("svg.lucide-settings").first().click();
    await page.locator('[data-settings-tab="system"]').click();
    await page
      .getByText(/Firewall mode is active/)
      .first()
      .waitFor({ timeout: 60000 });
  }

  async function closeSettings() {
    await page.getByRole("button", { name: "Close" }).first().click();
    await page
      .locator('[data-settings-tab="system"]')
      .waitFor({ state: "hidden" });
  }

  async function videoMenu() {
    await page.locator("svg.lucide-monitor").first().click();
    const trigger = page.getByText("Video Mode", { exact: true });
    if (mobile) await trigger.click();
    else await trigger.hover();
    await page.locator("[data-video-mode]").first().waitFor();
    // The menu reads the firewall status when it opens.
    await page.waitForTimeout(3000);
    const transports = await page
      .locator("[data-video-mode]")
      .evaluateAll((items) =>
        items.map((item) => ({
          mode: item.getAttribute("data-video-mode"),
          selectable: item.getAttribute("aria-disabled") !== "true",
        })),
      );
    await page.keyboard.press("Escape");
    await page.mouse.click(5, 5);
    return transports;
  }

  async function latency() {
    const sample = async (count) => {
      const times = [];
      for (let index = 0; index < count; index++) {
        times.push((await api("/api/vm/mouse-jiggler")).ms);
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      times.sort((a, b) => a - b);
      return {
        p50: times[Math.floor(times.length / 2)],
        max: times.at(-1),
        count,
      };
    };
    const idle = await sample(15);
    // Past the server's status cache, so the owner reads the rules afresh.
    await new Promise((resolve) => setTimeout(resolve, 6000));
    const reading = api("/api/system/firewall");
    const during = await sample(15);
    const status = await reading;
    record("latency", {
      idle,
      duringFirewallRead: during,
      firewallReadMs: status.ms,
      firewallCode: status.body.code,
    });
  }

  for (const step of steps) {
    if (step === "latency") {
      await latency();
      continue;
    }
    const before = writes.length;
    await openFirewall();
    await page
      .getByRole("button", { name: new RegExp(`^${title(step)}`) })
      .first()
      .click();
    await page
      .getByRole("button", { name: applyLabel[step], exact: true })
      .click();
    await page
      .getByText(`${title(step)} Firewall mode is active`)
      .first()
      .waitFor({ timeout: 60000 });
    // Leave time for the page's own confirmation to land.
    await page.waitForTimeout(3000);
    const owner = (await api("/api/system/firewall/mode")).body.data;
    const entry = {
      writes: writes.slice(before),
      ownerMode: owner?.mode,
      pending: owner?.pendingOperationId ?? null,
    };
    await closeSettings();
    if (step === "restricted" || step === "paranoid")
      entry.videoMenu = await videoMenu();
    record(`mode_${step}`, entry);
    assert.equal(entry.ownerMode, step);
    assert.equal(entry.pending, null);
    if (entry.videoMenu)
      assert.ok(
        entry.videoMenu.every(
          ({ mode, selectable }) =>
            selectable === !["h264", "vp8"].includes(mode),
        ),
        "a WebRTC transport stayed selectable",
      );
  }
  result = "passed";
} catch (error) {
  record("error", { message: String(error?.message ?? error).slice(0, 300) });
} finally {
  receipt.result = result;
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1), { mode: 0o600 });
  await browser.close();
}
console.log(`result: ${result}`);
process.exit(result === "passed" ? 0 : 1);
