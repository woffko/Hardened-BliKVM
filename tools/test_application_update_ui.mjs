import assert from "node:assert/strict";
import fs from "node:fs";
import ts from "../web/node_modules/typescript/lib/typescript.js";

async function load(relative) {
  const source = fs.readFileSync(new URL(relative, import.meta.url), "utf8");
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: {
      target: ts.ScriptTarget.ES2020,
      module: ts.ModuleKind.ESNext,
    },
  });
  return import(
    `data:text/javascript;base64,${Buffer.from(outputText).toString("base64")}`
  );
}

const { isApplicationUpdateStatus: valid } = await load(
  "../web/src/lib/application-update.ts",
);
const { supportsSettingsTab } = await load("../web/src/lib/capabilities.ts");
const slot = (sequence) => ({
  slotId: `app-${sequence.padStart(20, "0")}-${"a".repeat(64)}`,
  version: "0.1.0-dev2",
  sequence,
  minimumSystemSequence: "1",
});
const fixture = () => ({
  schemaVersion: 1,
  generation: "10",
  floorSequence: "2",
  systemSequence: "1",
  selected: slot("2"),
  confirmed: slot("2"),
  previous: slot("3"),
  pending: null,
  lastRollback: "boot_changed",
});
assert.equal(valid(fixture()), true);
const large = fixture();
large.generation = "18446744073709551615";
assert.equal(valid(large), true);
for (const value of [10, "0", "01", "18446744073709551616"]) {
  assert.equal(valid({ ...fixture(), generation: value }), false);
}
assert.equal(valid({ ...fixture(), path: "/untrusted" }), false);
assert.equal(valid({ ...fixture(), selected: slot("3") }), false);
const pending = {
  ...fixture(),
  selected: slot("3"),
  pending: {
    operationId: "b".repeat(32),
    target: slot("3"),
    phase: "awaiting_health",
  },
};
assert.equal(valid(pending), true);
assert.equal(
  valid({
    ...pending,
    pending: { ...pending.pending, operationId: "0".repeat(32) },
  }),
  false,
);
assert.equal(
  valid({ ...fixture(), confirmed: { ...slot("2"), version: "<script>" } }),
  false,
);
const readonly = { availability: "read_only", evidence: "source_verified" };
const unavailable = { availability: "unavailable", evidence: "none" };
const manifest = {
  platform: { id: "bli-h616-pikvm" },
  system: {
    signedApplicationUpdates: readonly,
    signedSystemUpdates: unavailable,
  },
};
assert.equal(supportsSettingsTab(manifest, "update"), true);
assert.equal(
  supportsSettingsTab(
    { ...manifest, platform: { id: "legacy-platform" } },
    "update",
  ),
  false,
);
assert.equal(supportsSettingsTab(null, "update"), false);
console.log("application update UI contract: passed");
