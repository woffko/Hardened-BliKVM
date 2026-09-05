import assert from "node:assert/strict";
import test from "node:test";
import { validateStatus } from "./playwright/status_api_acceptance.mjs";

function fixture() {
  const slot = {
    slotId:
      "app-00000000000000000004-768b33ac3a022f4a216ae1db4a08e7926802eabb4842b0de1dc80c74aa70095c",
    version: "0.1.0-dev3",
    sequence: "4",
    minimumSystemSequence: "1",
  };
  return {
    schemaVersion: 1,
    generation: "16",
    floorSequence: "4",
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
    { floorSequence: 4 },
    { generation: "17" },
    { pending: {} },
    { lastRollback: "health_timeout" },
  ]) {
    assert.throws(() => validateStatus({ ...fixture(), ...override }));
  }
  const wrong = fixture();
  wrong.selected = { ...wrong.selected, sequence: "2" };
  assert.throws(() => validateStatus(wrong));
});
