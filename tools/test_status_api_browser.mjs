import assert from "node:assert/strict";
import test from "node:test";
import {
  validateStatus,
  verifyStatusPanel,
} from "./playwright/status_api_acceptance.mjs";

function fixture() {
  const slot = {
    slotId:
      "app-00000000000000000005-41cb7620d36a47c3416e748296fa1512fc59aaf9012f12f04c0f659098629ebc",
    version: "0.1.0-dev4",
    sequence: "5",
    minimumSystemSequence: "1",
  };
  return {
    schemaVersion: 1,
    generation: "22",
    floorSequence: "5",
    systemSequence: "1",
    selected: slot,
    confirmed: { ...slot },
    pending: null,
    lastRollback: null,
  };
}

test("status acceptance binds exact string counters and confirmed slot", () => {
  validateStatus(fixture());
  for (const override of [
    { floorSequence: 5 },
    { floorSequence: "4" },
    { generation: "23" },
    { pending: {} },
    { lastRollback: "health_timeout" },
  ]) {
    assert.throws(() => validateStatus({ ...fixture(), ...override }));
  }
  const wrong = fixture();
  wrong.selected = { ...wrong.selected, sequence: "2" };
  assert.throws(() => validateStatus(wrong));
});

function panelPage({ hidden = false, revealWorks = true } = {}) {
  const events = [];
  let width = 1280;
  const locator = (kind) => ({
    first() {
      return this;
    },
    async boundingBox() {
      return { x: 700, y: hidden ? -20 : 20, width: 18, height: 18 };
    },
    async click() {
      events.push(kind);
      if (kind === "svg.lucide-settings" && width === 1280 && hidden) {
        throw new Error("toolbar still hidden");
      }
    },
    async waitFor() {},
    async count() {
      return kind === "0.1.0-dev4 · #5" ? 2 : 0;
    },
    locator,
    getByText: locator,
    getByRole: (_role, options) => locator(options?.name || "dialog"),
    getByTestId: locator,
  });
  return {
    events,
    async evaluate() {
      return { status: 200, body: { code: 0, data: fixture() } };
    },
    async setViewportSize(viewport) {
      width = viewport.width;
    },
    locator,
    getByRole: () => locator("dialog"),
    mouse: {
      async move(x, y) {
        events.push("reveal");
        assert.equal(x, 640);
        assert.equal(y, 1);
        if (revealWorks) hidden = false;
      },
    },
    async waitForResponse() {
      return { status: () => 200, json: async () => ({ data: fixture() }) };
    },
  };
}

test("status panel reveals the idle toolbar before normal settings clicks", async () => {
  const page = panelPage({ hidden: true });
  const steps = [];
  const result = await verifyStatusPanel(page, true, (step) =>
    steps.push(step),
  );
  assert.equal(result.desktopSettingsOffscreenBeforeReveal, true);
  assert.equal(result.desktop, true);
  assert.equal(result.mobile, true);
  assert.ok(
    page.events.indexOf("reveal") < page.events.indexOf("svg.lucide-settings"),
  );
  assert.equal(page.events.filter((event) => event === "reveal").length, 1);
  assert.equal(steps.at(-1), "complete");
});

test("a broken reveal fails at the named action without bypassing the UI", async () => {
  let lastStep;
  await assert.rejects(
    verifyStatusPanel(
      panelPage({ hidden: true, revealWorks: false }),
      true,
      (step) => {
        lastStep = step;
      },
    ),
    /toolbar still hidden/,
  );
  assert.equal(lastStep, "desktop-open-settings");
});
