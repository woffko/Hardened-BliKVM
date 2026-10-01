// Read-only walk of the GUI on a board: every menu popover is opened and each
// of its submenus hovered, every dialog and settings tab (and System section)
// opened. Nothing inside is clicked, so no setting, power line, HID report or
// media command is sent. Every /api request is recorded with its HTTP status
// and response code, as are console and page errors; a step fails when an
// API answer is an error or the page reports one. The GUI password is read
// from stdin; a receipt is written under evidence/private. No screenshot is
// taken.
//
// Usage: node gui_walk_device.mjs BASE_URL USERNAME OUTPUT VIEWPORT
//   VIEWPORT: desktop | mobile
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [baseUrl, username, outputValue, viewport] = process.argv.slice(2);
assert.equal(new URL(baseUrl).protocol, "https:");
const output = path.resolve(outputValue);
assert.equal(path.dirname(output), path.resolve("evidence/private"));
assert.ok(!fs.existsSync(output));
assert.ok(["desktop", "mobile"].includes(viewport));
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
  probe: "gui-walk-device",
  viewport,
  steps: [],
};
let step = { step: "load", api: [], errors: [] };
const flush = (next) => {
  receipt.steps.push(step);
  const failed = step.api.filter((call) => call.problem);
  console.log(
    `${step.step}: ${step.api.length} api, ${failed.length} failed, ${step.errors.length} errors`,
  );
  for (const call of failed)
    console.log(
      `   ${call.method} ${call.path} -> ${call.status} ${call.code ?? ""} ${call.msg ?? ""}`,
    );
  for (const error of step.errors) console.log(`   error: ${error}`);
  step = { step: next, api: [], errors: [] };
};

const page = await context.newPage();
page.setDefaultTimeout(8000);
page.on("console", (message) => {
  if (message.type() === "error")
    step.errors.push(`console: ${message.text().slice(0, 200)}`);
});
page.on("pageerror", (error) =>
  step.errors.push(`page: ${String(error.message).slice(0, 200)}`),
);
page.on("response", async (response) => {
  const url = new URL(response.url());
  if (!url.pathname.startsWith("/api/")) return;
  const request = response.request();
  const call = {
    method: request.method(),
    path:
      url.pathname +
      (url.search ? "?" + [...url.searchParams.keys()].join("&") : ""),
    status: response.status(),
  };
  try {
    const type = response.headers()["content-type"] ?? "";
    if (type.includes("json")) {
      const body = await response.json();
      call.code = body?.code;
      if (body?.code !== 0) call.msg = String(body?.msg ?? "").slice(0, 120);
      if (
        body?.data &&
        typeof body.data === "object" &&
        body.data.applied === false
      )
        call.rejection = body.data.rejection ?? null;
    }
  } catch {
    // Streams and bodies already consumed are recorded by status only.
  }
  call.problem =
    call.status >= 400 || (call.code !== undefined && call.code !== 0);
  step.api.push(call);
});
page.on("requestfailed", (request) => {
  const url = new URL(request.url());
  if (!url.pathname.startsWith("/api/")) return;
  if (/stream|ws|webrtc|mjpeg/.test(url.pathname)) return;
  step.api.push({
    method: request.method(),
    path: url.pathname,
    status: 0,
    msg: request.failure()?.errorText,
    problem: true,
  });
});

const settle = (ms = 1500) => page.waitForTimeout(ms);
const closeOverlays = async () => {
  await page.keyboard.press("Escape").catch(() => {});
  await page.mouse
    .click(mobile ? 380 : 1400, mobile ? 830 : 980)
    .catch(() => {});
  await settle(500);
};

// On a phone the menu bar hides after five seconds without a touch; its tab
// at the top edge brings it back.
const revealMenu = async () => {
  const tab = page.getByTestId("menu-reveal");
  if (await tab.count()) {
    await tab.click().catch(() => {});
    await settle(500);
  }
};

let result = "failed";
try {
  const login = await context.request.post(`${baseUrl}/api/auth/login`, {
    data: { username, password },
    headers: { Origin: baseUrl },
  });
  password = "";
  assert.ok((await login.json()).data?.csrfToken, "login failed");
  await page.goto(`${baseUrl}/`, { waitUntil: "domcontentloaded" });
  await page.locator("svg.lucide-settings").first().waitFor({ timeout: 30000 });
  await settle(4000);
  flush("menu");

  const bar = page.locator("div.rounded.bg-neutral-800\\/80").first();
  const labels = await bar.locator(":scope > *").evaluateAll((items) =>
    items.map((item) =>
      [...item.querySelectorAll("svg")]
        .map(
          (svg) =>
            (svg.getAttribute("class") ?? "").match(/lucide-([a-z0-9-]+)/)?.[1],
        )
        .filter(Boolean)
        .join("+"),
    ),
  );
  receipt.menu = labels.filter(Boolean);
  // Toggles and view controls are not opened: they act on click.
  const skip = new Set([
    "grip-vertical",
    "shield",
    "shield-check",
    "keyboard+smartphone",
    "maximize",
    "minimize",
    "chevron-right",
    "x",
    "",
  ]);
  const dialogs = new Set([
    "disc",
    "download",
    "square-terminal",
    "file-json",
    "network",
    "settings",
  ]);
  for (const [index, label] of labels.entries()) {
    if (skip.has(label) || label === "settings") continue;
    step.step = `menu:${label}`;
    const item = bar.locator(":scope > *").nth(index);
    await revealMenu();
    await item.locator("svg").first().click();
    await settle();
    if (!dialogs.has(label)) {
      // A popover: hover (desktop) or tap (mobile) each submenu trigger.
      const triggers = page.locator(
        ".ant-popover:visible [data-menu-keep-open]",
      );
      const count = await triggers.count();
      for (let at = 0; at < count; at++) {
        const trigger = triggers.nth(at);
        const name = (await trigger.innerText().catch(() => ""))
          .trim()
          .slice(0, 40);
        if (mobile) await trigger.click().catch(() => {});
        else await trigger.hover().catch(() => {});
        await settle(1200);
        step.api.push({ note: `submenu ${name}` });
      }
    }
    await closeOverlays();
    flush(`next`);
  }

  // Settings: every tab, and every section of System.
  step.step = "settings";
  await revealMenu();
  await page.locator("svg.lucide-settings").first().click();
  await page.locator("[data-settings-tab]").first().waitFor();
  await settle();
  const tabs = await page
    .locator("[data-settings-tab]")
    .evaluateAll((items) =>
      items.map((item) => item.getAttribute("data-settings-tab")),
    );
  receipt.settingsTabs = tabs;
  flush("next");
  for (const tab of tabs) {
    step.step = `settings:${tab}`;
    await page.locator(`[data-settings-tab="${tab}"]`).click();
    await settle(2500);
    if (tab === "system") {
      const sections = page
        .locator(".ant-modal-wrap")
        .filter({ has: page.locator("[data-settings-tab]") })
        .locator(".ant-segmented")
        .first()
        .locator(".ant-segmented-item");
      const names = await sections.allInnerTexts();
      flush("next");
      for (const [at, name] of names.entries()) {
        step.step = `settings:system:${name.trim()}`;
        await sections.nth(at).click();
        await settle(3000);
        flush("next");
      }
      continue;
    }
    flush("next");
  }
  await page
    .getByRole("button", { name: "Close" })
    .first()
    .click()
    .catch(() => {});
  await settle();
  flush("done");
  receipt.steps.pop();
  const problems = receipt.steps.flatMap((entry) =>
    entry.api
      .filter((call) => call.problem)
      .map(
        (call) =>
          `${entry.step}: ${call.method} ${call.path} ${call.status} ${call.code ?? ""}`,
      ),
  );
  const errors = receipt.steps.flatMap((entry) =>
    entry.errors.map((error) => `${entry.step}: ${error}`),
  );
  receipt.problems = problems;
  receipt.errors = errors;
  result = problems.length === 0 && errors.length === 0 ? "passed" : "failed";
} catch (error) {
  step.errors.push(`walk: ${String(error?.message ?? error).slice(0, 300)}`);
  receipt.steps.push(step);
} finally {
  receipt.result = result;
  fs.writeFileSync(output, JSON.stringify(receipt, null, 1), { mode: 0o600 });
  await browser.close();
}
console.log(`result: ${result}`);
process.exit(result === "passed" ? 0 : 1);
