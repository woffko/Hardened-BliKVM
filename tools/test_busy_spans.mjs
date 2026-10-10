// The spans of time behind the quality monitor's count of how busy the
// page's threads are (web/src/lib/busy.ts).
import assert from "node:assert/strict";
import test from "node:test";

const { BUSY_SPAN_MS, Spans } = await import("../web/src/lib/busy.ts");

test("spans that meet are counted once, and only within the last second", () => {
  const spans = new Spans();
  // An idle period of 10 ms, said twice by two callbacks inside it.
  spans.add(0, 10);
  spans.add(5, 10);
  spans.add(20, 30);
  assert.equal(spans.share(1000), 20 / BUSY_SPAN_MS);
  // Half of the first span is older than a second at 1005.
  assert.equal(spans.share(1005), 15 / BUSY_SPAN_MS);
  // A span still going on counts up to now.
  spans.add(1990, 2050);
  assert.equal(spans.share(2000), 10 / BUSY_SPAN_MS);
  // Nothing beyond the whole second.
  const busy = new Spans();
  busy.add(0, 5000);
  assert.equal(busy.share(3000), 1);
  // An empty or backwards span is nothing.
  const none = new Spans();
  none.add(10, 10);
  none.add(20, 15);
  assert.equal(none.share(100), 0);
});

test("only the last two seconds are kept", () => {
  const spans = new Spans();
  for (let at = 0; at < 10_000; at += 20) spans.add(at, at + 10);
  assert.ok(spans.spans.length <= 110, `${spans.spans.length} spans kept`);
  assert.equal(spans.share(10_000), 0.5);
});
