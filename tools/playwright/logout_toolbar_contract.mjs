// Logout on the toolbar (docs/baselines/work-plan-20261117-system-log.md, L5), through the built page: a button after
// Settings asks as Settings > Account does; cancelled, nothing is sent; confirmed, the session is ended once and the
// login page shown; the Appearance list hides it like the other toolbar items.
import assert from "node:assert/strict";
import { startApp } from "./app_harness.mjs";
import { eventually } from "./eventually.mjs";
import { launchChromium } from "./quality_fixture.mjs";
import { serveStillPicture } from "./take_control.mjs";

const app = await startApp();
const browser = await launchChromium();

try {
  for (const hidden of [false, true]) {
    const context = await browser.newContext({
      viewport: { width: 1280, height: 800 },
    });
    await context.addCookies([
      { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
    ]);
    if (hidden)
      await context.addInitScript(() => {
        // about:blank has no storage of its own.
        try {
          localStorage.setItem(
            "hardened-blikvm-menu-disabled-items",
            JSON.stringify(["logout"]),
          );
        } catch {
          // nothing to hide there
        }
      });
    const page = await context.newPage();
    page.setDefaultTimeout(8000);
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto("about:blank");
    await serveStillPicture(page);
    let logouts = 0;
    await page.route("**/api/auth/logout", (route) => {
      logouts += 1;
      return route.fulfill({ json: { code: 0, msg: "success", data: null } });
    });
    await page.goto(app.baseUrl, { waitUntil: "load" });
    await page.locator("svg.lucide-settings").first().waitFor();
    const button = page.getByTestId("toolbar-logout");

    if (hidden) {
      await page.waitForTimeout(500);
      assert.equal(await button.count(), 0, "a hidden logout button was shown");
    } else {
      await button.waitFor();
      // After Settings, before full screen.
      const order = await page.evaluate(() => {
        const settings = document
          .querySelector("svg.lucide-settings")
          .getBoundingClientRect().left;
        const logout = document
          .querySelector('[data-testid="toolbar-logout"]')
          .getBoundingClientRect().left;
        return logout > settings;
      });
      assert.ok(order, "the logout button is not after Settings");
      await button.click();
      await page.getByText("Are you sure you want to logout?").waitFor();
      await page.getByRole("button", { name: "No", exact: true }).click();
      await page.waitForTimeout(300);
      assert.equal(logouts, 0, "a cancelled logout was sent");
      await button.click();
      await page.getByRole("button", { name: "Yes", exact: true }).click();
      assert.ok(
        await eventually(() => page.url().endsWith("/auth/login"), 3000),
        `not on the login page: ${page.url()}`,
      );
      assert.equal(logouts, 1);
    }
    assert.deepEqual(errors, [], errors.join("\n"));
    await context.close();
  }
  console.log(
    "logout toolbar contract: ok (the button after Settings asked; cancelled, nothing was sent; confirmed, one logout and the login page; hidden by the Appearance list)",
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
