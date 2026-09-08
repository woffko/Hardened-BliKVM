import assert from "node:assert/strict";
import { runStatusSoak } from "./playwright/status_soak.mjs";

const selected = {
  slotId:
    "app-00000000000000000005-41cb7620d36a47c3416e748296fa1512fc59aaf9012f12f04c0f659098629ebc",
  version: "0.1.0-dev4",
  sequence: "5",
  minimumSystemSequence: "1",
};
const response = {
  status: 200,
  body: {
    code: 0,
    data: {
      schemaVersion: 1,
      generation: "22",
      floorSequence: "5",
      systemSequence: "1",
      pending: null,
      lastRollback: null,
      selected,
      confirmed: selected,
    },
  },
};
async function attempt(read) {
  let clock = 0;
  let completed = 0;
  const result = await runStatusSoak(read, {
    samples: 3,
    intervalMs: 10,
    now: () => clock,
    sleep: async (duration) => {
      clock += duration;
    },
    progress: (value) => {
      completed = value.completedSamples;
    },
  });
  assert.equal(completed, 3);
  return result;
}
assert.equal(
  (await attempt(async () => structuredClone(response))).elapsedMs,
  20,
);
for (const mutate of [
  (value) => {
    value.status = 503;
  },
  (value) => {
    value.body.data.generation = 22;
  },
  (value) => {
    value.body.data.floorSequence = "6";
  },
  (value) => {
    value.body.data.pending = {};
  },
  (value) => {
    value.body.data.extra = "changed";
  },
]) {
  let reads = 0;
  await assert.rejects(
    attempt(async () => {
      const value = structuredClone(response);
      if (++reads === 2) mutate(value);
      return value;
    }),
  );
  assert.equal(reads, 2);
}
await assert.rejects(
  attempt(async () => {
    throw new Error("transport failure");
  }),
);
await assert.rejects(runStatusSoak(async () => response, { samples: 100 }));
await assert.rejects(
  runStatusSoak(async () => response, {
    samples: 2,
    intervalMs: 1,
    now: (() => {
      let clock = 0;
      return () => (clock += 5001);
    })(),
  }),
);
console.log(
  "status soak: stable counters, sequential failure, transport, bounds and latency passed",
);
