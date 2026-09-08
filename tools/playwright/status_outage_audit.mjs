import assert from "node:assert/strict";
import { statusRoute } from "./status_api_acceptance.mjs";

export function createStatusOutageWindow(clock = () => performance.now()) {
  let started = null;
  let ended = false;
  let accepted = 0;
  const requestToken = Symbol("status-outage-request");
  const limit = 90_000;
  return {
    begin() {
      assert.equal(started, null, "status outage window already used");
      started = clock();
      assert.ok(Number.isFinite(started));
    },
    captureRequest() {
      if (started === null || ended) return null;
      const elapsed = clock() - started;
      return elapsed >= 0 && elapsed <= limit ? requestToken : null;
    },
    allows(response, token = null) {
      if (started === null || token !== requestToken) return false;
      const elapsed = clock() - started;
      const allowed =
        elapsed >= 0 &&
        elapsed <= limit &&
        response.method === "GET" &&
        response.route === statusRoute &&
        response.status === 503;
      if (allowed) accepted += 1;
      return allowed;
    },
    end() {
      assert.notEqual(started, null, "status outage window was not opened");
      assert.equal(ended, false, "status outage window already closed");
      const elapsed = clock() - started;
      assert.ok(
        elapsed >= 0 && elapsed <= limit,
        "status outage window exceeded its bound",
      );
      ended = true;
    },
    assertCompleted() {
      assert.equal(ended, true, "status outage window remains open");
      assert.ok(accepted > 0, "no expected status unavailability was observed");
    },
  };
}

export function validateOutageResult(value) {
  assert.ok(value && typeof value === "object" && !Array.isArray(value));
  const names = [
    "staleVerified",
    "coldErrorVerified",
    "recoveryVerified",
    "videoPreserved",
  ];
  assert.deepEqual(Object.keys(value).sort(), names.sort());
  for (const name of names) assert.equal(value[name], true);
}
