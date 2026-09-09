import assert from "node:assert/strict";
import test from "node:test";
import {
  statusProfile,
  validateStatus,
} from "./playwright/status_api_acceptance.mjs";
import {
  validateActivation,
  validateActivationRequest,
  validateStaged,
  waitLoginInputs,
} from "./playwright/application_write_acceptance.mjs";

test("login waits for the lazy form instead of using document title as readiness", async () => {
  let visible = false;
  const inputs = {
    nth(index) {
      assert.equal(index, 1);
      return {
        async waitFor(options) {
          assert.deepEqual(options, { state: "visible" });
          visible = true;
        },
      };
    },
    async count() {
      return visible ? 2 : 0;
    },
  };
  const page = {
    locator(selector) {
      assert.equal(selector, "input");
      return inputs;
    },
  };
  assert.equal(await waitLoginInputs(page), inputs);
});

function staged() {
  const target = statusProfile("seq8");
  return {
    code: 0,
    data: {
      slot: {
        slotId: target.slot,
        version: target.version,
        sequence: "8",
        minimumSystemSequence: "1",
      },
      disposition: "already_present",
      manifestSha256: target.slot.slice(25),
      payloadBytes: "8683648",
    },
  };
}

test("device receipt requires exact retained sequence-eight archive and string counters", () => {
  validateStaged(staged());
  for (const changes of [
    { disposition: "published" },
    { payloadBytes: 8683648 },
    { manifestSha256: "foreign" },
  ]) {
    assert.throws(() =>
      validateStaged({ code: 0, data: { ...staged().data, ...changes } }),
    );
  }
});

test("activation request stays bound to reviewed generation and target", () => {
  const body = { slotId: statusProfile("seq8").slot, expectedGeneration: "37" };
  validateActivationRequest(body);
  for (const changes of [
    { expectedGeneration: 37 },
    { expectedGeneration: "38" },
    { slotId: statusProfile("seq7").slot },
    { force: true },
  ]) {
    assert.throws(() => validateActivationRequest({ ...body, ...changes }));
  }
});

test("awaiting-health is a validated receipt, never confirmed success", () => {
  const body = {
    code: 0,
    data: {
      phase: "awaiting_health",
      receipt: {
        operationId: "a".repeat(32),
        targetSlotId: statusProfile("seq8").slot,
      },
    },
  };
  validateActivation(body);
  assert.throws(() =>
    validateActivation({ ...body, data: { ...body.data, phase: "confirmed" } }),
  );
  assert.throws(() =>
    validateActivation({
      ...body,
      data: {
        ...body.data,
        receipt: { ...body.data.receipt, operationId: "0".repeat(32) },
      },
    }),
  );
});

test("post-restoration status profile requires confirmed seq8 generation40", () => {
  const slot = staged().data.slot;
  const value = {
    schemaVersion: 1,
    generation: "40",
    floorSequence: "8",
    systemSequence: "1",
    selected: slot,
    confirmed: slot,
    pending: null,
    lastRollback: null,
  };
  validateStatus(value, "seq8");
  assert.throws(() => validateStatus(value, "seq7"));
  assert.throws(() => validateStatus({ ...value, pending: {} }, "seq8"));
  assert.throws(() => validateStatus({ ...value, generation: "39" }, "seq8"));
});
