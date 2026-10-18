// "Start the agent" (web/src/lib/agent-start.ts, Settings > Device), through the built page with a played board whose
// agent's switch is on and no agent connected. Without remote control the button only says to enable it; with it,
// a confirmation comes first, then the keyboard's reports: Win+R, a pause for the Run box, the line at the US layout's
// positions key by key, Enter, the line a second after Win+R was sent however late the lease sent it. An agent
// that does not come in 20 s leaves the start offered again with what to look
// at; when the agent says hello, the page shows it connected. An agent of another commit than the board's is said to
// be so and offered an update, typed the same way and waited for until the board's agent is the connected one; an
// agent of the board's commit is offered nothing.
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

// The SHA-256 of the board's agent, which the line checks before it runs
// anything (the security review of 2026-10-18).
const DIGEST =
  "5387551047879ddf4d2d3dac19f65f86ceaa01cb20bdc2fe6d7fe5e4f1eeb929";

try {
  let connected = false;
  let agentSha256 = DIGEST;
  let agentCommit = "82af928c0ffee";
  let boardCommit;
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
  // The keyboard's reports as they leave for the board; the next one may be
  // held back, as a slow lease would.
  let holdNextReport = 0;
  const leaving = [];
  await page.route("**/api/hid/lease", async (route) => {
    const body = route.request().postDataJSON();
    if (body?.action === "report" && body.report?.mode === "keyboard") {
      if (holdNextReport > 0) {
        const held = holdNextReport;
        holdNextReport = 0;
        await new Promise((resolve) => setTimeout(resolve, held));
      }
      leaving.push({ data: body.report.data, at: performance.now() });
    }
    return route.continue();
  });
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
                commit: agentCommit,
                system: null,
              }
            : null,
          refused: null,
          messagesIn: 0,
          messagesOut: 0,
          dropped: 0,
          ...(boardCommit === undefined ? {} : { boardCommit }),
          agentSha256,
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

  // Without the board's agent's digest nothing is typed: the button says why.
  agentSha256 = "";
  await openDevice();
  await page
    .getByText("The board does not name its agent's checksum", { exact: false })
    .waitFor({ timeout: 12000 });
  assert.equal(await button.isDisabled(), true);
  agentSha256 = DIGEST;
  assert.ok(await eventually(async () => !(await button.isDisabled()), 12000));
  await page.keyboard.press("Escape");

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
  // Win+R is held back 800 ms on its way.
  const leftBefore = leaving.length;
  holdNextReport = 800;
  await dialog.getByRole("button", { name: "Type it" }).click();

  // Win+R, released, then the line and Enter, each key pressed and released.
  const expected = [];
  for (const step of [
    ...start.runBoxSteps(),
    ...start.commandSteps(start.agentStartCommand(DIGEST)),
  ]) {
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
  // The line's first key left at least a second after Win+R did.
  const typedNow = leaving.slice(leftBefore);
  const runBox = typedNow.findIndex(
    (report) => JSON.stringify(report.data) === JSON.stringify(expected[0]),
  );
  const line = typedNow.findIndex(
    (report, index) =>
      index > runBox + 1 &&
      JSON.stringify(report.data) === JSON.stringify(expected[2]),
  );
  assert.ok(runBox >= 0 && line > runBox, JSON.stringify(typedNow.slice(0, 4)));
  const waited = typedNow[line].at - typedNow[runBox].at;
  assert.ok(waited >= 990, `the line ${Math.round(waited)} ms after Win+R`);
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

  // The board names another commit: the agent is said not to be the
  // board's, and an update is offered, typed as the start is.
  boardCommit = "8dfb152d0c4b1e2f3a4b5c6d7e8f901234567890";
  const otherVersion = page.getByTestId("target-agent-other-version");
  await otherVersion.waitFor({ timeout: 12000 });
  const update = page.getByRole("button", {
    name: "Update the agent on the target",
  });
  assert.equal(await update.isDisabled(), false);
  const updating = keyboardReports().length;
  await update.click();
  const updateDialog = page.getByRole("dialog", {
    name: "Update the agent on the target",
  });
  await updateDialog.getByText("the agent running now leaves").waitFor();
  await updateDialog.getByRole("button", { name: "Type it" }).click();
  assert.ok(
    await eventually(
      () => keyboardReports().length >= updating + expected.length,
      20000,
    ),
  );
  await page.getByText("Typed; waiting for the agent to connect").waitFor();
  // The board's agent takes the place of the other: nothing more is said
  // or offered.
  agentCommit = boardCommit;
  await otherVersion.waitFor({ state: "detached", timeout: 12000 });
  assert.equal(await update.count(), 0);
  await page
    .getByText("Typed; waiting for the agent to connect")
    .waitFor({ state: "detached" });
  assert.equal(await page.getByRole("alert").count(), 0);
  assert.deepEqual(errors, []);
  await context.close();
  console.log(
    `target agent start contract: ok (${expected.length} keyboard reports)`,
  );
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
