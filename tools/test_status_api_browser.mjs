import assert from "node:assert/strict";
import test from "node:test";
import {
  validateStatus,
  verifyStatusPanel,
  statusProfile,
} from "./playwright/status_api_acceptance.mjs";

function fixture(profileName = "seq5") {
  const profile = statusProfile(profileName);
  const slot = {
    slotId: profile.slot,
    version: profile.version,
    sequence: profile.sequence,
    minimumSystemSequence: "1",
  };
  return {
    schemaVersion: 1,
    generation: profile.generation,
    floorSequence: profile.sequence,
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

test("sequence-six profile binds the confirmed slot and generation without relaxing sequence five", () => {
  assert.deepEqual(statusProfile("seq6"), {
    generation: "31",
    sequence: "6",
    version: "0.1.0-dev5",
    slot: "app-00000000000000000006-8eedeb5ce21ca08b90336cad27bdd7f16c6502388a1f13bc943a2026d80534ac",
  });
  validateStatus(fixture("seq6"), "seq6");
  assert.throws(() => validateStatus(fixture("seq6")));
  assert.throws(() => validateStatus(fixture(), "seq6"));
  assert.throws(() =>
    validateStatus({ ...fixture("seq6"), generation: "30" }, "seq6"),
  );
  assert.throws(() => statusProfile("unreviewed"));
  assert.throws(() => statusProfile("__proto__"));
});

test("sequence-six panel checks both viewports using its explicit profile", async () => {
  const result = await verifyStatusPanel(
    panelPage({ profileName: "seq6" }),
    true,
    undefined,
    "seq6",
  );
  assert.equal(result.desktop, true);
  assert.equal(result.mobile, true);
  assert.equal(result.refreshChecks, 2);
});

test("sequence-seven status and both panels bind the installed generation", async () => {
  assert.deepEqual(statusProfile("seq7"), {
    generation: "37",
    sequence: "7",
    version: "0.1.0-dev6",
    slot: "app-00000000000000000007-f2262e8f9929dda83b2baf1005b5ae9262aa075b7ddaa16f4ad6200b5daf0140",
  });
  validateStatus(fixture("seq7"), "seq7");
  assert.throws(() => validateStatus(fixture("seq6"), "seq7"));
  assert.throws(() => validateStatus(fixture("seq7"), "seq6"));
  assert.throws(() =>
    validateStatus({ ...fixture("seq7"), generation: "34" }, "seq7"),
  );
  const result = await verifyStatusPanel(
    panelPage({ profileName: "seq7" }),
    true,
    undefined,
    "seq7",
  );
  assert.equal(result.desktop, true);
  assert.equal(result.mobile, true);
  assert.equal(result.refreshChecks, 2);
});

function panelPage({
  hidden = false,
  revealWorks = true,
  profileName = "seq5",
} = {}) {
  const profile = statusProfile(profileName);
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
      return kind === `${profile.version} · #${profile.sequence}` ? 2 : 0;
    },
    locator,
    getByText: locator,
    getByRole: (_role, options) => locator(options?.name || "dialog"),
    getByTestId: locator,
  });
  return {
    events,
    async evaluate() {
      return { status: 200, body: { code: 0, data: fixture(profileName) } };
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
      return {
        status: () => 200,
        json: async () => ({ data: fixture(profileName) }),
      };
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
