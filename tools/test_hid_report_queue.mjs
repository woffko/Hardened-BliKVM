import assert from "node:assert/strict";
import test from "node:test";

import { hasWheel, mergeMouseReport } from "../web/src/lib/hid-report-queue.ts";

test("relative movement is added without losing signed deltas", () => {
  assert.deepEqual(
    mergeMouseReport(
      { mode: "relative_mouse", data: [1, 10, 251, 0] },
      { mode: "relative_mouse", data: [1, 3, 2, 0] },
    ),
    { mode: "relative_mouse", data: [1, 13, 253, 0] },
  );
});

test("overflow, buttons, and wheel force ordered reports", () => {
  assert.equal(
    mergeMouseReport(
      { mode: "relative_mouse", data: [0, 100, 0, 0] },
      { mode: "relative_mouse", data: [0, 40, 0, 0] },
    ),
    null,
  );
  assert.equal(
    mergeMouseReport(
      { mode: "relative_mouse", data: [0, 1, 0, 0] },
      { mode: "relative_mouse", data: [1, 1, 0, 0] },
    ),
    null,
  );
  const wheel = { mode: "relative_mouse", data: [0, 0, 0, 255] };
  assert.equal(hasWheel(wheel), true);
  assert.equal(mergeMouseReport(wheel, wheel), null);
});

test("absolute movement keeps the newest position for unchanged buttons", () => {
  const latest = { mode: "absolute_mouse", data: [1, 52, 18, 120, 86, 0] };
  assert.equal(
    mergeMouseReport(
      { mode: "absolute_mouse", data: [1, 1, 0, 2, 0, 0] },
      latest,
    ),
    latest,
  );
  assert.equal(
    hasWheel({ mode: "absolute_mouse", data: [0, 0, 0, 0, 0, 1] }),
    true,
  );
});
