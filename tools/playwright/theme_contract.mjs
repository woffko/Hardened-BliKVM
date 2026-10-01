// Colours of the interface: dark by default (the colours as before), light
// on request, the system's theme for "System" (followed live), both chosen in
// Settings > Appearance and kept in this browser, with an accent that reaches
// antd's primary colour and Tailwind's blue shades; what has the theme-fixed
// class keeps the dark values.
import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
import { startApp } from "./app_harness.mjs";

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

async function open(stored = {}, colorScheme = "dark") {
  const context = await browser.newContext({ colorScheme });
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  await context.addInitScript((values) => {
    if (sessionStorage.getItem("fixture-stored")) return;
    sessionStorage.setItem("fixture-stored", "1");
    for (const [key, value] of Object.entries(values))
      localStorage.setItem(key, value);
  }, stored);
  const page = await context.newPage();
  page.setDefaultTimeout(6000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto(app.baseUrl, { waitUntil: "networkidle" });
  return { context, page, errors };
}

const state = (page) =>
  page.evaluate(() => {
    const probe = document.createElement("div");
    probe.className = "theme-fixed";
    document.body.append(probe);
    const fixed = getComputedStyle(probe)
      .getPropertyValue("--color-neutral-900")
      .trim();
    probe.remove();
    return {
      theme: document.documentElement.dataset.theme,
      body: getComputedStyle(document.body).backgroundColor,
      accent: getComputedStyle(document.documentElement)
        .getPropertyValue("--color-accent-500")
        .trim(),
      fixed,
      storedTheme: localStorage.getItem("hardened-blikvm-theme"),
      storedAccent: localStorage.getItem("hardened-blikvm-accent"),
    };
  });

try {
  // Nothing chosen: dark, Tailwind's blue, the colours as before.
  {
    const { context, page, errors } = await open();
    const now = await state(page);
    assert.equal(now.theme, "dark");
    assert.equal(now.body, "rgb(10, 10, 10)");
    assert.equal(now.accent, "59 130 246");
    assert.deepEqual(errors, []);
    await context.close();
  }
  // Light, kept from an earlier visit; theme-fixed keeps the dark values.
  {
    const { context, page, errors } = await open({
      "hardened-blikvm-theme": "light",
      "hardened-blikvm-accent": "green",
    });
    const now = await state(page);
    assert.equal(now.theme, "light");
    assert.equal(now.body, "rgb(250, 250, 250)");
    assert.equal(now.accent, "34 197 94");
    assert.equal(now.fixed, "23 23 23");
    assert.deepEqual(errors, []);
    await context.close();
  }
  // System: follows the browser's scheme, also when it changes.
  {
    const { context, page, errors } = await open(
      { "hardened-blikvm-theme": "system" },
      "light",
    );
    assert.equal((await state(page)).theme, "light");
    await page.emulateMedia({ colorScheme: "dark" });
    await page.waitForFunction(
      () => document.documentElement.dataset.theme === "dark",
    );
    assert.deepEqual(errors, []);
    await context.close();
  }
  // Chosen in Settings > Appearance: applied at once and kept; the accent
  // reaches antd's primary colour.
  {
    const { context, page, errors } = await open();
    await page.locator("svg.lucide-settings").first().click();
    await page.getByText("Appearance", { exact: true }).first().click();
    await page.getByText("Light", { exact: true }).click();
    await page.waitForFunction(
      () => document.documentElement.dataset.theme === "light",
    );
    await page.locator('[data-accent="violet"]').click();
    const now = await state(page);
    assert.equal(now.storedTheme, "light");
    assert.equal(now.storedAccent, "violet");
    assert.equal(now.accent, "139 92 246");
    assert.equal(
      await page.locator('[data-accent="violet"]').getAttribute("aria-pressed"),
      "true",
    );
    const primary = await page.evaluate(() => {
      const probe = document.querySelector(".ant-segmented-item-selected");
      return probe ? getComputedStyle(probe).color : "";
    });
    assert.ok(primary, "a selected segment is styled");
    // The Submenu Icons switch is on by default: a checked antd switch takes
    // the primary colour.
    await page.waitForTimeout(800); // the switch's colour transition
    const switchColor = await page.evaluate(() => {
      const probe = document.querySelector(
        ".ant-modal-body .ant-switch-checked",
      );
      return probe ? getComputedStyle(probe).backgroundColor : "";
    });
    // antd derives the switch's shade from the primary colour.
    const [red, green, blue] = switchColor.match(/\d+/g).map(Number);
    assert.ok(
      Math.abs(red - 124) <= 12 &&
        Math.abs(green - 58) <= 12 &&
        Math.abs(blue - 237) <= 12,
      `antd primary follows the accent: ${switchColor}`,
    );
    assert.deepEqual(errors, []);
    await context.close();
  }
  console.log(
    "Theme: dark by default, light kept, system followed live, chosen in the settings, accent in antd and Tailwind, theme-fixed unchanged passed",
  );
} finally {
  await browser.close();
  app.server.close();
}
