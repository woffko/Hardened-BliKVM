// "Start the agent" (web/src/lib/agent-start.ts, Settings > Device), through the built page with a played board whose
// agent's switch is on and no agent connected. Without remote control the button only says to enable it; with it,
// a confirmation comes first, then the keyboard's reports: Win+R, a pause for the Run box, the line at the US layout's
// positions key by key, Enter. An agent that does not come in 20 s leaves the start offered again with what to look
// at; when the agent says hello, the page shows it connected.
import assert from "node:assert/strict";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { defaultManifest, startApp } from "./app_harness.mjs";
import { launchChromium } from "./quality_fixture.mjs";

const start = await import(
  pathToFileURL(
    path.resolve(import.meta.dirname, "../../web/src/lib/agent-start.ts"),
  ).href
);
const browser = await launchChromium();
const feature = (availability) => ({
  availability,
  evidence: "source_verified",
});
const app = await startApp({
  manifest: defaultManifest({
    board: {
      features: {
        device_settings: feature("read_only"),
        target_agent: feature("read_write"),
      },
    },
  }),
});
const keyboardReports = () =>
  app.requestDetails
    .filter(
      (request) =>
        request.route === "/api/hid/lease" &&
        request.body?.action === "report" &&
        request.body.report?.mode === "keyboard",
    )
    .map((request) => ({
      at: request.at ?? 0,
      data: request.body.report.data,
    }));

async function eventually(condition, timeoutMs = 8000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await condition()) return true;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  return condition();
}

try {
  let connected = false;
  const context = await browser.newContext({
    viewport: { width: 1280, height: 800 },
  });
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("**/api/target-agent", (route) =>
    route.fulfill({
      json: {
        code: 0,
        data: {
          available: true,
          enabled: true,
          open: true,
          problem: null,
          agent: connected
            ? {
                program: "hardened-blikvm-agent",
                version: "0.1.0",
                commit: "82af928c0ffee",
                system: null,
              }
            : null,
          refused: null,
          messagesIn: 0,
          messagesOut: 0,
          dropped: 0,
        },
      },
    }),
  );
  await page.goto(app.baseUrl, { waitUntil: "networkidle" });
  const openDevice = async () => {
    await page.locator("svg.lucide-settings").click();
    await page.locator('[data-settings-tab="device"]').click();
  };

  // Without remote control: the button waits for it.
  await openDevice();
  const button = page.getByRole("button", {
    name: "Start the agent on the target",
  });
  await button.waitFor();
  assert.equal(await button.isDisabled(), true);
  await page
    .getByText("Enable remote control to start the agent from here")
    .waitFor();
  await page.keyboard.press("Escape");

  await page.getByRole("button", { name: "Enable remote control" }).click();
  const confirm = page.getByRole("button", { name: "Confirm and control" });
  await confirm.click();
  await page.waitForTimeout(300);
  if (await confirm.isVisible()) await confirm.click();
  await confirm.waitFor({ state: "hidden" });

  // A confirmation first, which says what is typed.
  await openDevice();
  await button.click();
  const dialog = page.getByRole("dialog", {
    name: "Start the agent on the target",
  });
  await dialog.getByText("English (US) keyboard layout").waitFor();
  // Nothing is typed before the confirmation (enabling control may have
  // released the keys once).
  const before = keyboardReports().length;
  await page.waitForTimeout(300);
  assert.equal(keyboardReports().length, before);
  await dialog.getByRole("button", { name: "Type it" }).click();

  // Win+R, released, then the line and Enter, each key pressed and released.
  const expected = [];
  for (const step of [...start.runBoxSteps(), ...start.commandSteps()]) {
    expected.push(
      [step.modifier, 0, step.usage, 0, 0, 0, 0, 0],
      [0, 0, 0, 0, 0, 0, 0, 0],
    );
  }
  assert.ok(
    await eventually(
      () => keyboardReports().length >= before + expected.length,
      20000,
    ),
    `${keyboardReports().length - before} of ${expected.length} reports`,
  );
  assert.deepEqual(
    keyboardReports()
      .slice(before)
      .map((report) => report.data),
    expected,
  );
  await page.getByText("Typed; waiting for the agent to connect").waitFor();

  // No agent comes: after 20 s the start is offered again, with what to
  // look at on the target.
  await page
    .getByRole("alert")
    .getByText("The agent did not connect within 20 s", { exact: false })
    .waitFor({ timeout: 30000 });
  assert.equal(await button.isDisabled(), false);

  // Again, and this time the agent says hello: connected.
  const again = keyboardReports().length;
  await button.click();
  await dialog.getByRole("button", { name: "Type it" }).click();
  assert.ok(
    await eventually(
      () => keyboardReports().length >= again + expected.length,
      20000,
    ),
  );
  await page.getByText("Typed; waiting for the agent to connect").waitFor();
  assert.equal(await page.getByRole("alert").count(), 0);
  connected = true;
  await page
    .getByTestId("target-agent-status")
    .getByText("Connected: hardened-blikvm-agent 0.1.0 (82af928c0ffe)")
    .waitFor({ timeout: 12000 });
  assert.equal(await button.count(), 0);
  assert.deepEqual(errors, []);
  await context.close();
  console.log(
    `target agent start contract: ok (${expected.length} keyboard reports)`,
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
