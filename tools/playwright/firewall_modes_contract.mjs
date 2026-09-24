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
try {
  for (const [writable, accepted, mobile, rollback] of [
    [false, false, false],
    [true, false, false],
    [true, true, false],
    [true, true, true],
    [true, true, true, true],
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
    page.setDefaultTimeout(4000);
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
    await page.route("**/api/capabilities", (route) =>
      route.fulfill({ json: { code: 0, data: manifest } }),
    );
    let state = {
      mode: "moderate",
      pendingOperationId: null,
      confirmationDeadlineUnixMs: null,
      recoveryRequired: false,
      configurationReady: true,
    };
    const writes = [];
    await page.route("**/api/system/firewall/mode**", (route) => {
      const request = route.request();
      if (request.method() === "GET")
        return route.fulfill({ json: { code: 0, data: state } });
      const body = request.postDataJSON();
      writes.push({ path: new URL(request.url()).pathname, body });
      if (!accepted)
        return route.fulfill({
          json: {
            code: 0,
            data: {
              applied: false,
              rejection: "fixture_refused",
              operationId: "a".repeat(32),
            },
          },
        });
      if (request.url().endsWith("/confirm")) {
        assert.equal(body.operationId, state.pendingOperationId);
        state = {
          ...state,
          pendingOperationId: null,
          confirmationDeadlineUnixMs: null,
        };
      } else {
        assert.deepEqual(body, { mode: "restricted", confirm: true });
        state = {
          ...state,
          mode: "restricted",
          pendingOperationId: "a".repeat(32),
          confirmationDeadlineUnixMs: String(Date.now() + 60000),
        };
      }
      return route.fulfill({
        json: {
          code: 0,
          data: { applied: true, rejection: null, operationId: "a".repeat(32) },
        },
      });
    });
    await page.goto(app.baseUrl, { waitUntil: "networkidle" });
    await page.locator("svg.lucide-settings").click();
    await page.locator('[data-settings-tab="system"]').click();
    await page.getByText("Current mode: moderate", { exact: true }).waitFor();
    assert.equal(writes.length, 0);
    const restricted = page.getByRole("button", {
      name: "restricted",
      exact: true,
    });
    assert.equal(await restricted.isDisabled(), !writable);
    if (writable) {
      await restricted.click();
      await page.getByRole("button", { name: "Cancel", exact: true }).click();
      assert.equal(writes.length, 0, "Cancel caused a write");
      await restricted.click();
      await page.getByRole("button", { name: "Apply", exact: true }).click();
      if (accepted) {
        await page
          .getByText("Current mode: restricted", { exact: true })
          .waitFor();
        assert.equal(writes.length, 1, "confirmation must not be automatic");
        if (rollback) {
          state = {
            ...state,
            confirmationDeadlineUnixMs: String(Date.now() - 1),
            recoveryRequired: true,
          };
          await page
            .getByRole("alert")
            .filter({ hasText: "Firewall recovery is pending." })
            .waitFor();
          assert.equal(
            await page
              .getByRole("button", {
                name: "Confirm firewall connectivity",
                exact: true,
              })
              .isDisabled(),
            true,
          );
          assert.equal(
            await page
              .getByRole("button", { name: "baseline", exact: true })
              .isDisabled(),
            true,
          );
          state = {
            ...state,
            mode: "moderate",
            pendingOperationId: null,
            confirmationDeadlineUnixMs: null,
            recoveryRequired: false,
          };
          await page
            .getByText("Current mode: moderate", { exact: true })
            .waitFor();
          assert.equal(
            await page
              .getByRole("button", {
                name: "Confirm firewall connectivity",
                exact: true,
              })
              .count(),
            0,
          );
          assert.equal(
            writes.length,
            1,
            "browser performed a mutation during owner rollback",
          );
          assert.deepEqual(errors, []);
          await context.close();
          continue;
        }
        await page
          .getByRole("button", {
            name: "Confirm firewall connectivity",
            exact: true,
          })
          .click();
        await page
          .getByRole("button", {
            name: "Confirm firewall connectivity",
            exact: true,
          })
          .waitFor({ state: "hidden" });
        assert.equal(writes.length, 2);
        assert.deepEqual(writes[1], {
          path: "/api/system/firewall/mode/confirm",
          body: { operationId: "a".repeat(32) },
        });
      } else {
        await page
          .getByRole("alert")
          .filter({ hasText: "fixture_refused" })
          .waitFor();
        assert.equal(
          await page
            .getByText("Current mode: moderate", { exact: true })
            .count(),
          1,
        );
        assert.equal(writes.length, 1);
      }
    }
    assert.deepEqual(errors, []);
    await context.close();
  }
  console.log(
    "Firewall modes: read-only, Cancel, refusal, explicit desktop/mobile confirmation and owner rollback polling passed",
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
