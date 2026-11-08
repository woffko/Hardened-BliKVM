// The contracts' waits (tools/playwright/eventually.mjs): this host's wall
// clock steps by about 3.6 s every 32 s, so a wait whose deadline is taken
// from Date.now() can end before a timer it waits for
// (target_agent_reads_contract.mjs:431 failed so). The waits are timed by
// performance.now(), which does not step.
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { eventually, deadline } from "./playwright/eventually.mjs";

const PLAYWRIGHT = path.resolve(import.meta.dirname, "playwright");

/** The wait as the contracts had it. */
async function wallClockEventually(condition, timeoutMs) {
  const end = Date.now() + timeoutMs;
  while (Date.now() < end) {
    if (await condition()) return true;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  return condition();
}

/** Runs `body` while the wall clock steps 3.6 s ahead every 500 ms. */
async function withSteppingClock(body) {
  const real = Date.now;
  let offset = 0;
  Date.now = () => real() + offset;
  const stepping = setInterval(() => {
    offset += 3600;
  }, 500);
  try {
    return await body();
  } finally {
    clearInterval(stepping);
    Date.now = real;
  }
}

test("a step of the wall clock does not end a wait early", async () => {
  await withSteppingClock(async () => {
    let early = false;
    setTimeout(() => {
      early = true;
    }, 3000);
    assert.equal(
      await wallClockEventually(() => early, 6000),
      false,
      "the wall-clock wait was expected to give up",
    );
    let done = false;
    setTimeout(() => {
      done = true;
    }, 3000);
    assert.equal(await eventually(() => done, 6000), true);
  });
});

test("a deadline passes on the steady clock", async () => {
  const passed = deadline(200);
  assert.equal(passed(), false);
  await new Promise((resolve) => setTimeout(resolve, 250));
  assert.equal(passed(), true);
  assert.equal(await eventually(() => false, 150), false);
});

test("no wait under tools/playwright is timed by Date.now()", () => {
  const timed =
    /deadline\s*=\s*Date\.now\(\)|Date\.now\(\)\s*\+\s*(timeoutMs|\d)/;
  const found = readdirSync(PLAYWRIGHT)
    .filter((name) => name.endsWith(".mjs"))
    .filter((name) =>
      timed.test(readFileSync(path.join(PLAYWRIGHT, name), "utf8")),
    );
  assert.deepEqual(found, []);
});
