import assert from "node:assert/strict";
import test from "node:test";
import { writeProfile } from "./playwright/application_write_profiles.mjs";

test("seq9 retry is distinct from initial seq9 publication", () => {
  const retry = writeProfile("seq9-retry");
  assert.equal(retry.source, "seq9");
  assert.equal(retry.target, "seq9");
  assert.equal(retry.disposition, "already_present");
  assert.equal(retry.bundleSha256, writeProfile("seq8-to9").bundleSha256);
  assert.notEqual(retry.disposition, writeProfile("seq8-to9").disposition);
});
import {
  statusProfile,
  validateStatus,
} from "./playwright/status_api_acceptance.mjs";
import {
  validateActivation,
  validateActivationRequest,
  validateStaged,
  waitLoginInputs,
  validateUploadBody,
} from "./playwright/application_write_acceptance.mjs";

test("seq8-to9 profile binds new archive, publication and generation40", () => {
  const profile = writeProfile("seq8-to9");
  assert.equal(profile.disposition, "published");
  assert.notEqual(profile.bundleSha256, writeProfile().bundleSha256);
  const target = statusProfile("seq9");
  validateActivationRequest(
    { slotId: target.slot, expectedGeneration: "40" },
    "seq8-to9",
  );
  assert.throws(() =>
    validateActivationRequest(
      { slotId: target.slot, expectedGeneration: "37" },
      "seq8-to9",
    ),
  );
  assert.throws(() =>
    validateActivationRequest(
      { slotId: statusProfile("seq8").slot, expectedGeneration: "40" },
      "seq8-to9",
    ),
  );
  const body = {
    code: 0,
    data: {
      slot: {
        slotId: target.slot,
        version: target.version,
        sequence: "9",
        minimumSystemSequence: "1",
      },
      disposition: "published",
      manifestSha256: target.slot.slice(25),
      payloadBytes: "8683648",
    },
  };
  validateStaged(body, "seq8-to9");
  assert.throws(() => validateStaged(body));
  assert.throws(() =>
    validateStaged(
      { ...body, data: { ...body.data, disposition: "already_present" } },
      "seq8-to9",
    ),
  );
  assert.throws(() => writeProfile("seq10-to11"));
});

test("seq9-to10 and retry bind corrected payload and generation43", () => {
  const profile = writeProfile("seq9-to10");
  const target = statusProfile("seq10");
  assert.equal(target.generation, "46");
  assert.equal(target.version, "0.1.0-dev7");
  assert.equal(
    profile.bundleSha256,
    "85e06d1b68e54480bab89c3cdc5e0d9f80cc3ee007e7ba1e425432af4b250f1f",
  );
  validateActivationRequest(
    { slotId: target.slot, expectedGeneration: "43" },
    "seq9-to10",
  );
  assert.throws(() =>
    validateActivationRequest(
      { slotId: target.slot, expectedGeneration: "40" },
      "seq9-to10",
    ),
  );
  const body = {
    code: 0,
    data: {
      slot: {
        slotId: target.slot,
        version: target.version,
        sequence: "10",
        minimumSystemSequence: "1",
      },
      disposition: "published",
      manifestSha256: target.slot.slice(25),
      payloadBytes: "8683976",
    },
  };
  validateStaged(body, "seq9-to10");
  assert.throws(() =>
    validateStaged(
      { ...body, data: { ...body.data, payloadBytes: "8683648" } },
      "seq9-to10",
    ),
  );
  assert.throws(() => validateStaged(body, "seq10-retry"));
  validateStaged(
    { ...body, data: { ...body.data, disposition: "already_present" } },
    "seq10-retry",
  );
  assert.equal(writeProfile("seq10-retry").bundleSha256, profile.bundleSha256);
});

test("omitted File-backed XHR body requires prior selected-file verification", () => {
  assert.equal(validateUploadBody(null, true), false);
  assert.throws(() => validateUploadBody(null, false));
  assert.throws(() => validateUploadBody(null, undefined));
  assert.throws(() => validateUploadBody(Buffer.from("foreign"), true));
});

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
