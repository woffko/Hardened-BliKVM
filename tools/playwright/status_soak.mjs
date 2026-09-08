import assert from "node:assert/strict";
import { setTimeout } from "node:timers/promises";
import { performance } from "node:perf_hooks";
import { validateStatus } from "./status_api_acceptance.mjs";

// Sequential reads: no overlapping requests and no catch-and-continue on failure.
export async function runStatusSoak(
  read,
  {
    samples = 61,
    intervalMs = 10000,
    now = () => performance.now(),
    sleep = setTimeout,
    progress = () => {},
  } = {},
) {
  assert.ok(Number.isInteger(samples) && samples >= 2 && samples <= 61);
  assert.ok(
    Number.isInteger(intervalMs) && intervalMs >= 1 && intervalMs <= 10000,
  );
  const started = now();
  let first = null;
  let maximumLatencyMs = 0;
  for (let index = 0; index < samples; index++) {
    if (index) await sleep(intervalMs);
    const before = now();
    const response = await read();
    const latency = now() - before;
    assert.ok(latency >= 0 && latency <= 5000, "status latency exceeded bound");
    assert.equal(response.status, 200);
    assert.equal(response.body.code, 0);
    validateStatus(response.body.data);
    if (first) assert.deepEqual(response.body.data, first);
    first = response.body.data;
    maximumLatencyMs = Math.max(maximumLatencyMs, latency);
    progress({ completedSamples: index + 1 });
  }
  const elapsedMs = now() - started;
  assert.ok(elapsedMs >= (samples - 1) * intervalMs);
  return {
    samples,
    intervalMs,
    elapsedMs: Math.round(elapsedMs),
    maximumLatencyMs: Math.ceil(maximumLatencyMs),
    exactStatusStable: true,
  };
}
