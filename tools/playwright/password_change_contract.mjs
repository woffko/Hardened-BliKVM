// Changing the password asks for the current one (MED-04 of the security
// review of 2026-10-18): the page sends it with the new password, says when
// the board refuses it as wrong, and goes to the login once the change is
// made.
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

try {
  const context = await browser.newContext({ locale: "en-US" });
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("**/api/auth/account", (route) =>
    route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        code: 0,
        data: { username: "operator", csrfToken: "fixture-csrf" },
      }),
    }),
  );
  const sent = [];
  let wrong = true;
  await page.route("**/api/auth/password", (route) => {
    if (route.request().method() !== "POST") return route.fallback();
    sent.push(route.request().postDataJSON());
    return route.fulfill(
      wrong
        ? {
            status: 403,
            contentType: "application/json",
            body: JSON.stringify({
              code: -403,
              msg: "the current password is wrong",
            }),
          }
        : {
            contentType: "application/json",
            body: JSON.stringify({ code: 0 }),
          },
    );
  });

  await page.goto(`${app.baseUrl}/#/auth/password`, {
    waitUntil: "networkidle",
  });
  const inputs = page.locator("input");
  assert.equal(await inputs.count(), 4, "user name, current and new twice");
  await assert.doesNotReject(async () => {
    await page.getByPlaceholder("Current password").waitFor();
  });
  await page.getByPlaceholder("Current password").fill("not the password");
  await inputs.nth(2).fill("second password");
  await inputs.nth(3).fill("second password");
  await page.getByRole("button", { name: "Ok" }).click();
  await page.getByText("The current password is wrong").waitFor();
  assert.deepEqual(sent[0], {
    username: "operator",
    password: "second password",
    currentPassword: "not the password",
  });
  assert.match(page.url(), /#\/auth\/password$/, "the page stays");

  wrong = false;
  await page.getByPlaceholder("Current password").fill("initial password");
  await page.getByRole("button", { name: "Ok" }).click();
  await page.waitForURL(/#\/auth\/login$/);
  assert.equal(sent.length, 2);
  assert.equal(sent[1].currentPassword, "initial password");
  assert.deepEqual(errors, []);
  await context.close();
  console.log(
    "password change contract: ok (the current password sent, a wrong one said, the login after the change)",
  );
} finally {
  await browser.close();
  app.server.close();
}
