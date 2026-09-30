// The Content-Security-Policy nginx sends (read from the shipped
// configuration) lets the page run: no violation is reported while it loads
// and while its menus open, on desktop and on a phone. Anything the policy
// blocked would show up as a `securitypolicyviolation` event.
import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
import {
  defaultManifest,
  shippedSecurityHeaders,
  startApp,
} from "./app_harness.mjs";

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
const headers = shippedSecurityHeaders();
const policy = headers["Content-Security-Policy"];
assert.ok(policy, "nginx sends a Content-Security-Policy");
for (const directive of [
  "default-src 'self'",
  "script-src 'self'",
  "object-src 'none'",
  "frame-ancestors 'none'",
  "base-uri 'self'",
])
  assert.ok(policy.includes(directive), directive);
assert.ok(!policy.includes("unsafe-eval"), "no eval");
assert.ok(!/script-src[^;]*unsafe-inline/.test(policy), "no inline script");
assert.equal(headers["X-Content-Type-Options"], "nosniff");
assert.equal(headers["Referrer-Policy"], "no-referrer");

try {
  for (const mobile of [false, true]) {
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
    await context.addInitScript(() => {
      window.__violations = [];
      document.addEventListener("securitypolicyviolation", (event) =>
        window.__violations.push(
          `${event.violatedDirective} ${event.blockedURI}`,
        ),
      );
    });
    const page = await context.newPage();
    const consoleProblems = [];
    page.on("console", (message) => {
      if (/Content Security Policy/i.test(message.text()))
        consoleProblems.push(message.text());
    });
    await page.route("**/api/capabilities", (route) =>
      route.fulfill({ json: { code: 0, data: defaultManifest() } }),
    );
    const response = await page.goto(app.baseUrl, { waitUntil: "networkidle" });
    assert.equal(response.headers()["content-security-policy"], policy);
    await page.locator("img#screen, #root *").first().waitFor();
    // Open a menu or two so lazily loaded chunks and styles are exercised.
    for (const icon of ["svg.lucide-monitor", "svg.lucide-settings"]) {
      const target = page.locator(icon).first();
      if (await target.count()) {
        await target.click().catch(() => {});
        await page.waitForTimeout(300);
      }
    }
    assert.deepEqual(
      await page.evaluate(() => window.__violations),
      [],
      `policy violations (${mobile ? "mobile" : "desktop"})`,
    );
    assert.deepEqual(consoleProblems, []);
    await context.close();
  }
  console.log("CSP contract: no violation on desktop and mobile");
} finally {
  await browser.close();
  app.server.close();
}
