// The target agent's switch in Settings > Device (web/src/pages/desktop/menu/settings/device/target-agent.tsx,
// docs/baselines/work-plan-20261014-target-agent.md), through the built page with a played board. The switch shows
// the board's state and reaches the board only after a confirmation that says the USB device leaves the target and
// comes back; a cancelled confirmation sends nothing. Switched on, the page shows the connected agent; a channel the
// board cannot open shows why. A board without the feature has no switch, one that offers it read-only a disabled
// one, and the switch speaks Russian. The settings show the page's one state of the agent: opened, they add no read of
// their own.
import assert from "node:assert/strict";
import { defaultManifest, startApp } from "./app_harness.mjs";
import { launchChromium } from "./quality_fixture.mjs";

const browser = await launchChromium();
const app = await startApp();
const feature = (availability) => ({
  availability,
  evidence: "source_verified",
});
const AGENT = {
  program: "hardened-blikvm-agent",
  version: "0.1.0",
  commit: "3c940a7e681c00ff",
};

/** The played board: its switch, whether an agent says hello, and the page's writes. */
function playedBoard({ problem = null } = {}) {
  const board = { enabled: false, writes: [], reads: [] };
  board.state = () => ({
    available: true,
    enabled: board.enabled,
    open: board.enabled && !problem,
    problem: board.enabled ? problem : null,
    agent: board.enabled && !problem ? AGENT : null,
    refused: null,
    messagesIn: 0,
    messagesOut: 0,
    dropped: 0,
  });
  return board;
}

async function open(board, { access = "read_write", language = null } = {}) {
  const context = await browser.newContext({
    viewport: { width: 1280, height: 800 },
  });
  await context.addCookies([
    { name: "hardened-blikvm-csrf", value: "fixture-csrf", url: app.baseUrl },
  ]);
  await context.addInitScript((language) => {
    localStorage.setItem("hardened-blikvm-menu-auto-hide", "never");
    if (language) localStorage.setItem("hardened-blikvm-language", language);
  }, language);
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const features = { device_settings: feature("read_only") };
  if (access) features.target_agent = feature(access);
  const manifest = defaultManifest({ board: { features } });
  await page.route("**/api/capabilities", (route) =>
    route.fulfill({ json: { code: 0, data: manifest } }),
  );
  await page.route("**/api/target-agent", async (route) => {
    if (route.request().method() === "POST") {
      const { enabled } = route.request().postDataJSON();
      board.writes.push(enabled);
      board.enabled = enabled;
    } else board.reads.push(performance.now());
    await route.fulfill({ json: { code: 0, data: board.state() } });
  });
  await page.goto(app.baseUrl, { waitUntil: "networkidle" });
  await page.locator("svg.lucide-settings").click();
  await page.locator('[data-settings-tab="device"]').click();
  return { context, page, errors };
}

try {
  {
    const board = playedBoard();
    const { context, page, errors } = await open(board);
    const toggle = page.getByRole("switch", {
      name: "Target agent",
      exact: true,
    });
    await toggle.waitFor();
    assert.equal(await toggle.isChecked(), false);
    assert.equal(await toggle.isDisabled(), false);
    assert.equal(await page.getByTestId("target-agent-status").count(), 0);
    // Open for 11 s, the settings add no read of the state to the page's
    // own one every 5 s.
    const opened = performance.now();
    await page.waitForTimeout(11000);
    const reads = board.reads.filter((at) => at >= opened).length;
    assert.ok(reads <= 3, `${reads} reads of the agent's state in 11 s`);

    // A cancelled confirmation sends nothing.
    await toggle.click();
    const dialog = page.getByRole("dialog", {
      name: "Target agent",
      exact: true,
    });
    await dialog
      .getByText("leaves the target and comes back with the agent's channel")
      .waitFor();
    await dialog.getByRole("button", { name: "No", exact: true }).click();
    await dialog.waitFor({ state: "hidden" });
    assert.deepEqual(board.writes, []);
    assert.equal(await toggle.isChecked(), false);

    // Confirmed: the board switches and the agent's hello shows.
    await toggle.click();
    await dialog.getByRole("button", { name: "Switch", exact: true }).click();
    await page
      .getByTestId("target-agent-status")
      .getByText("Connected: hardened-blikvm-agent 0.1.0 (3c940a7e681c)", {
        exact: true,
      })
      .waitFor();
    assert.deepEqual(board.writes, [true]);
    assert.equal(await toggle.isChecked(), true);

    // Off again, also after a confirmation.
    await toggle.click();
    await dialog.getByText("comes back without the agent's channel").waitFor();
    await dialog.getByRole("button", { name: "Switch", exact: true }).click();
    await page
      .getByTestId("target-agent-status")
      .waitFor({ state: "detached" });
    assert.deepEqual(board.writes, [true, false]);
    assert.deepEqual(errors, []);
    await context.close();
  }
  {
    // A channel the board cannot open says why.
    const board = playedBoard({
      problem: "the board's system does not let the owner open /dev/hidg1",
    });
    board.enabled = true;
    const { context, page, errors } = await open(board);
    await page
      .getByText(
        "The channel is not open: the board's system does not let the owner open /dev/hidg1",
        {
          exact: true,
        },
      )
      .waitFor();
    assert.deepEqual(errors, []);
    await context.close();
  }
  {
    // Without the feature there is no switch; read-only, it is disabled.
    const board = playedBoard();
    let { context, page } = await open(board, { access: null });
    await page.getByText("Device", { exact: true }).first().waitFor();
    assert.equal(
      await page.getByRole("switch", { name: "Target agent" }).count(),
      0,
    );
    await context.close();
    ({ context, page } = await open(board, { access: "read_only" }));
    const toggle = page.getByRole("switch", {
      name: "Target agent",
      exact: true,
    });
    await toggle.waitFor();
    assert.equal(await toggle.isDisabled(), true);
    await context.close();
  }
  {
    // Russian.
    const board = playedBoard();
    const { context, page } = await open(board, { language: "ru" });
    await page
      .getByRole("switch", { name: "Агент на цели", exact: true })
      .click();
    await page
      .getByRole("dialog", { name: "Агент на цели", exact: true })
      .getByText("USB-устройство платы отключится от цели")
      .waitFor();
    await context.close();
  }
  console.log("target agent contract: ok");
} finally {
  await browser.close();
  await new Promise((resolve) => app.server.close(resolve));
}
