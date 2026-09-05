import assert from "node:assert/strict";
import test from "node:test";
import { validateStatus } from "./playwright/status_api_acceptance.mjs";

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
