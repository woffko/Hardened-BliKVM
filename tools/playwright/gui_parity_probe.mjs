import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [modulePath, executablePath, targetUrl, outputValue, usernameFile] =
  process.argv.slice(2);
assert.ok(
  modulePath && executablePath && targetUrl && outputValue,
  "missing arguments",
);

const url = new URL(targetUrl);
assert.ok(
  ["http:", "https:", "file:"].includes(url.protocol),
  "unsupported URL scheme",
);

const outputPath = path.resolve(outputValue);
assert.equal(
  path.dirname(outputPath),
  path.resolve("evidence/private"),
  "output must be under evidence/private",
);

const { chromium } = await import(pathToFileURL(modulePath).href);
const browser = await chromium.launch({ executablePath, headless: true });

try {
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    locale: "en-US",
  });
  const page = await context.newPage();
  const failedRequests = [];
  page.on("response", (response) => {
    if (failedRequests.length >= 20) return;
    const status = response.status();
    if (status < 400) return;
    const request = response.request();
    const route = new URL(response.url()).pathname;
    if (!route.startsWith("/api/")) {
      if (
        request.resourceType() === "image" ||
        request.resourceType() === "font"
      )
        return;
    }
    failedRequests.push({
      method: request.method(),
      route,
      status,
    });
  });

  let consoleErrors = 0;
  const consoleErrorSamples = [];
  page.on("console", (message) => {
    if (message.type() === "error") {
      consoleErrors += 1;
      if (consoleErrorSamples.length < 5) {
        const text = message.text().replace(/\s+/g, " ").trim().slice(0, 80);
        if (text) {
          consoleErrorSamples.push(text);
        }
      }
    }
  });

  await page.goto(targetUrl, { waitUntil: "load", timeout: 20_000 });
  await page.waitForTimeout(500);

  if (usernameFile) {
    const username = fs.readFileSync(usernameFile, "utf8").trim();
    let password = "";
    for await (const chunk of process.stdin) {
      password += chunk.toString("utf8");
      if (password.length > 4096) {
        throw new Error("password input exceeded the limit");
      }
    }
    password = password.replace(/[\r\n]+$/, "");
    const userInput = await page.$(
      'input[type="text"], input[type="email"], input[name*="user" i]',
    );
    const passInput = await page.$('input[type="password"]');
    if (userInput && passInput && password) {
      await userInput.fill(username);
      await passInput.fill(password);
      const submit = await page.$('button[type="submit"], button');
      if (submit) {
        await submit.click();
      } else {
        await passInput.press("Enter");
      }
      await page
        .waitForLoadState("networkidle", { timeout: 20_000 })
        .catch(() => {});
      await page.waitForTimeout(800);
    }
  }

  const tooltipLabels = [];
  try {
    const buttons = (
      await page.$$('button, [role="button"], header svg')
    ).slice(0, 30);
    for (const button of buttons) {
      await button.hover({ timeout: 1_000 }).catch(() => {});
      await page.waitForTimeout(120);
      const texts = await page.$$eval(
        '.ant-tooltip-inner, [role="tooltip"], .tooltip',
        (nodes) =>
          nodes
            .filter((node) => {
              const style = window.getComputedStyle(node);
              return style.visibility !== "hidden" && style.display !== "none";
            })
            .map((node) => (node.textContent || "").trim())
            .filter((text) => text && text.length <= 80),
      );
      tooltipLabels.push(...texts);
    }
  } catch {
    /* tooltip sweep is best effort */
  }

  const dom = await page.evaluate(() => {
    const labels = [];
    const seen = new Set();
    const nodes = document.querySelectorAll(
      '[role="menuitem"], [role="button"], nav a, nav button, header button, ' +
        ".ant-menu-item, .ant-dropdown-menu-item, [aria-label], [title]",
    );
    for (const node of nodes) {
      const text = (
        node.textContent ||
        node.getAttribute("aria-label") ||
        node.getAttribute("title") ||
        ""
      )
        .trim()
        .replace(/\s+/g, " ");
      if (text && text.length <= 80 && !seen.has(text)) {
        seen.add(text);
        labels.push(text);
      }
    }

    const tagCounts = {};
    for (const element of document.querySelectorAll("*")) {
      const tag = element.tagName.toLowerCase();
      tagCounts[tag] = (tagCounts[tag] || 0) + 1;
    }
    const bounded = Object.fromEntries(
      Object.entries(tagCounts)
        .sort(([left], [right]) => (left < right ? -1 : 1))
        .slice(0, 40),
    );

    return {
      title: document.title || "",
      route: `${location.pathname}${location.hash}`,
      menuLabels: labels.slice(0, 100),
      tagCounts: bounded,
    };
  });

  const mergedLabels = [];
  const seenLabels = new Set();
  for (const label of [...dom.menuLabels, ...tooltipLabels]) {
    if (!seenLabels.has(label)) {
      seenLabels.add(label);
      mergedLabels.push(label);
    }
  }

  let capabilitiesAvailable = false;
  const capabilities = {};
  try {
    if (url.protocol === "http:" || url.protocol === "https:") {
      const capabilityResponse = await context.request.get(
        new URL("/api/capabilities", url).href,
      );
      if (capabilityResponse.ok()) {
        capabilitiesAvailable = true;
        const body = await capabilityResponse.json();
        const data = body?.data ?? body;
        const walk = (node, prefix) => {
          if (
            !node ||
            typeof node !== "object" ||
            Object.keys(capabilities).length >= 60
          ) {
            return;
          }
          for (const [key, value] of Object.entries(node)) {
            const keyPath = prefix ? `${prefix}.${key}` : key;
            if (keyPath.length > 80) continue;
            if (typeof value === "string") {
              if (value.length <= 24) capabilities[keyPath] = value;
            } else if (
              typeof value === "boolean" ||
              typeof value === "number"
            ) {
              capabilities[keyPath] = String(value);
            } else if (value && typeof value === "object") {
              walk(value, keyPath);
            }
          }
        };
        walk(data, "");
      }
    }
  } catch {
    capabilitiesAvailable = false;
  }

  const settingsTabs = [];
  try {
    // The menu auto-hides, so hover the top-centre strip to expand it first.
    const viewport = page.viewportSize() ?? { width: 1280, height: 720 };
    await page.mouse.move(Math.round(viewport.width / 2), 30);
    await page.waitForTimeout(500);
    const settingsIcon = page.locator("svg.lucide-settings").first();
    await settingsIcon.click({ force: true, timeout: 5000 });
    await page.waitForTimeout(600);
    const texts = await page.$$eval(
      '[role="tab"], .ant-tabs-tab, .ant-tabs-tab-btn',
      (nodes) =>
        nodes
          .map((node) => (node.textContent || "").trim())
          .filter((text) => text && text.length <= 40)
          .slice(0, 20),
    );
    for (const text of texts) {
      if (!settingsTabs.includes(text)) settingsTabs.push(text);
    }
  } catch {
    /* the settings surface is best effort */
  }

  const markers = await page.evaluate(() => ({
    mobileKeyboardInput: document.querySelectorAll(
      'textarea[aria-label="Mobile local keyboard input"]',
    ).length,
    onScreenKeyboardKeys: document.querySelectorAll(".hg-button").length,
    videoElements: document.querySelectorAll("img, canvas").length,
  }));

  const receipt = {
    schemaVersion: 1,
    probe: "hardened-blikvm-gui-parity",
    target: { scheme: url.protocol.replace(":", ""), route: dom.route },
    title: dom.title,
    menuLabels: mergedLabels.slice(0, 100),
    tagCounts: dom.tagCounts,
    capabilitiesAvailable,
    capabilities,
    settingsTabs,
    markers,
    consoleErrors,
    consoleErrorSamples,
    failedRequests,
    cookiesRead: false,
    storageRead: false,
    mutationsPerformed: false,
  };
  fs.writeFileSync(outputPath, `${JSON.stringify(receipt, null, 2)}\n`, {
    encoding: "utf8",
    mode: 0o600,
  });
  console.log(
    `gui-parity target=${url.protocol}//${url.host} route=${dom.route} labels=${receipt.menuLabels.length} consoleErrors=${consoleErrors}`,
  );
} finally {
  await browser.close();
}
