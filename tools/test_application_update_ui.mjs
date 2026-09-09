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

const {
  isApplicationUpdateStatus: valid,
  isApplicationUploadReceipt: validReceipt,
  validApplicationArchiveSize: validSize,
  eligibleApplicationTarget: eligible,
  currentApplicationIntent: currentIntent,
  isApplicationActivationReceipt: validActivation,
  reconcileApplicationActivation: reconcile,
} = await load("../web/src/lib/application-update.ts");
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
const receipt = {
  slot: slot("18446744073709551615"),
  disposition: "published",
  manifestSha256: "a".repeat(64),
  payloadBytes: "134217728",
};
assert.equal(validReceipt(receipt), true);
assert.equal(
  validReceipt({ ...receipt, disposition: "already_present" }),
  true,
);
for (const value of [
  null,
  {},
  { ...receipt, path: "/tmp/archive" },
  { ...receipt, disposition: "activated" },
  { ...receipt, manifestSha256: "b".repeat(64) },
  { ...receipt, payloadBytes: 1 },
  { ...receipt, payloadBytes: "0" },
  { ...receipt, payloadBytes: "134217729" },
  { ...receipt, slot: { ...receipt.slot, sequence: "1" } },
]) {
  assert.equal(validReceipt(value), false);
}
for (const value of [1, 134217728]) assert.equal(validSize(value), true);
for (const value of [0, -1, 0.5, 134217729, NaN, Infinity, "1"])
  assert.equal(validSize(value), false);
const intent = {
  target: slot("3"),
  current: slot("2"),
  expectedGeneration: "10",
};
const activation = {
  phase: "awaiting_health",
  receipt: { operationId: "b".repeat(32), targetSlotId: slot("3").slotId },
};
assert.equal(eligible(slot("3"), fixture()), true);
assert.equal(eligible(slot("2"), fixture()), false);
assert.equal(
  eligible({ ...slot("3"), minimumSystemSequence: "2" }, fixture()),
  false,
);
assert.equal(eligible(slot("3"), pending), false);
assert.equal(currentIntent(intent, fixture()), true);
assert.equal(currentIntent(intent, { ...fixture(), generation: "11" }), false);
assert.equal(validActivation(activation, slot("3").slotId), true);
for (const invalid of [
  null,
  {},
  { ...activation, confirmed: true },
  { ...activation, phase: "confirmed" },
  {
    ...activation,
    receipt: { ...activation.receipt, healthToken: "forbidden" },
  },
  {
    ...activation,
    receipt: { ...activation.receipt, operationId: "0".repeat(32) },
  },
]) {
  assert.equal(validActivation(invalid, slot("3").slotId), false);
}
assert.equal(validActivation(activation, slot("4").slotId), false);
assert.equal(reconcile(intent, activation, fixture()), "awaiting");
assert.equal(reconcile(intent, null, fixture()), "uncertain");
assert.equal(
  reconcile(intent, activation, { ...pending, generation: "11" }),
  "awaiting",
);
assert.equal(
  reconcile(intent, null, { ...pending, generation: "11" }),
  "awaiting",
);
assert.equal(
  reconcile(intent, activation, {
    ...pending,
    generation: "11",
    pending: { ...pending.pending, operationId: "c".repeat(32) },
  }),
  "changed",
);
assert.equal(
  reconcile(intent, activation, { ...fixture(), generation: "12" }),
  "notApplied",
);
assert.equal(
  reconcile(intent, null, {
    ...fixture(),
    generation: "12",
    confirmed: slot("3"),
    selected: slot("3"),
    floorSequence: "3",
  }),
  "installed",
);
assert.equal(
  currentIntent(
    { ...intent, expectedGeneration: "9007199254740992" },
    { ...fixture(), generation: "9007199254740993" },
  ),
  false,
);
console.log("application update UI contract: passed");
