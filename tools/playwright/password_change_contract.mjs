// Changing the password asks for the current one (MED-04 of the security
// review of 2026-10-18): the page sends it with the new password, says when
// the board refuses it as wrong, and goes to the login once the change is
// made.
import assert from "node:assert/strict";
import { startApp } from "./app_harness.mjs";
import { launchChromium } from "./quality_fixture.mjs";

const browser = await launchChromium();
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
  let answer = "wrong";
  await page.route("**/api/auth/password", (route) => {
    if (route.request().method() !== "POST") return route.fallback();
    sent.push(route.request().postDataJSON());
    const answers = {
      wrong: {
        status: 403,
        contentType: "application/json",
        body: JSON.stringify({
          code: -403,
          msg: "the current password is wrong",
        }),
      },
      // The limiter's answer, as the server gives it: 200 with code -5.
      locked: {
        contentType: "application/json",
        body: JSON.stringify({
          code: -5,
          msg: "account locked after repeated failures",
        }),
      },
      done: {
        contentType: "application/json",
        body: JSON.stringify({ code: 0 }),
      },
    };
    return route.fulfill(answers[answer]);
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

  answer = "locked";
  await page.getByRole("button", { name: "Ok" }).click();
  await page.getByText("Too many logins, please try again later").waitFor();

  answer = "done";
  await page.getByPlaceholder("Current password").fill("initial password");
  await page.getByRole("button", { name: "Ok" }).click();
  await page.waitForURL(/#\/auth\/login$/);
  assert.equal(sent.length, 3);
  assert.equal(sent[2].currentPassword, "initial password");
  assert.deepEqual(errors, []);
  await context.close();
  console.log(
    "password change contract: ok (the current password sent, a wrong one and the limiter said, the login after the change)",
  );
} finally {
  await browser.close();
  app.server.close();
}
