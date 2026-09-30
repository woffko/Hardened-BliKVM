import assert from "node:assert/strict";
import test from "node:test";

import {
  hasWheel,
  mergeMouseReport,
  MouseQueue,
} from "../web/src/lib/hid-report-queue.ts";

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

// A send that finishes only when the test lets it.
function slowSender() {
  const sent = [];
  const waiting = [];
  const send = (report) =>
    new Promise((resolve) => {
      sent.push(report);
      waiting.push(resolve);
    });
  const finish = async () => {
    waiting.shift()?.();
    await new Promise((resolve) => setTimeout(resolve, 0));
  };
  return { sent, send, finish };
}

const at = (x, buttons = 0) => ({
  mode: "absolute_mouse",
  data: [buttons, x & 0xff, 0, 0, 0, 0],
});
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

test("movement during a slow request goes out as one report", async () => {
  const { sent, send, finish } = slowSender();
  const queue = new MouseQueue(send, 1);
  queue.push(at(1));
  await wait(5);
  assert.equal(sent.length, 1);
  // The request is slow; the hand keeps moving.
  for (let x = 2; x <= 60; x += 1) queue.push(at(x));
  await wait(5);
  assert.equal(sent.length, 1, "nothing else is sent while one is in flight");
  await finish();
  assert.equal(sent.length, 2);
  assert.deepEqual(sent[1], at(60), "the newest position, not a backlog");
  await finish();
  assert.equal(sent.length, 2);
});

test("button changes are neither merged nor reordered", async () => {
  const { sent, send, finish } = slowSender();
  const queue = new MouseQueue(send, 1);
  queue.push(at(1));
  await wait(5);
  queue.push(at(2));
  queue.push(at(2, 1));
  queue.push(at(3, 1));
  queue.push(at(3, 0));
  const wheel = { mode: "absolute_mouse", data: [0, 3, 0, 0, 0, 1] };
  queue.push(wheel);
  for (let i = 0; i < 6; i += 1) await finish();
  assert.deepEqual(sent, [at(1), at(2), at(3, 1), at(3, 0), wheel]);
});

test("a cleared queue sends nothing more", async () => {
  const { sent, send, finish } = slowSender();
  const queue = new MouseQueue(send, 1);
  queue.push(at(1));
  await wait(5);
  queue.push(at(2));
  queue.clear();
  await finish();
  await wait(5);
  assert.deepEqual(sent, [at(1)]);
});
