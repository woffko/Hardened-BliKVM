// The upscaler's watchdog (web/src/lib/upscale/watchdog.ts,
// docs/design/screen-upscaling-v1.md): upscaling stops when fewer than 90 %
// of the frames received over five seconds were drawn, and not before five
// seconds were counted, nor on the few frames of a still screen. The file is
// TypeScript, which Node reads by stripping its types; where this Node
// cannot, the tests are skipped and say so.
import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

const readsTypeScript = Boolean(process.features.typescript);
const skip = readsTypeScript
  ? false
  : "this Node cannot read TypeScript (process.features.typescript)";
const watchdog = readsTypeScript
  ? await import(
      pathToFileURL(
        path.resolve(import.meta.dirname, "../web/src/lib/upscale/watchdog.ts"),
      ).href
    )
  : null;

/**
 * Plays `seconds` of frames at `fps`, each drawn `delay` ms after it came
 * unless `drawn(index)` says no; asks the watchdog every second. Returns the
 * second it first said frames were lost, or null.
 */
function play({
  fps,
  seconds,
  drawn = () => true,
  delay = 10,
  start = 0,
  dog,
}) {
  const one = dog ?? new watchdog.UpscaleWatchdog(start);
  const events = [];
  for (let index = 0; index < fps * seconds; index += 1) {
    const at = start + (index * 1000) / fps;
    events.push([at, "receive"]);
    if (drawn(index)) events.push([at + delay, "draw"]);
  }
  for (let second = 1; second <= seconds; second += 1) {
    events.push([start + second * 1000 + 0.5, "ask", second]);
  }
  events.sort((left, right) => left[0] - right[0]);
  for (const [at, what, second] of events) {
    if (what === "receive") one.receive(at);
    else if (what === "draw") one.draw(at);
    else if (one.losing(at)) return second;
  }
  return null;
}

test("every frame drawn: upscaling goes on", { skip }, () => {
  assert.equal(play({ fps: 60, seconds: 30 }), null);
  assert.equal(play({ fps: 15, seconds: 30, delay: 60 }), null);
});

test("a frame in ten lost is still within the share", { skip }, () => {
  // 90 % drawn: not fewer than 90 %.
  assert.equal(
    play({ fps: 30, seconds: 30, drawn: (index) => index % 10 !== 0 }),
    null,
  );
});

test("fewer than 90 % drawn for five seconds stops it", { skip }, () => {
  // A slow draw: one frame in five.
  assert.equal(
    play({ fps: 30, seconds: 30, drawn: (index) => index % 5 === 0 }),
    5,
  );
  // A decoder that falls behind and is started again: a second's frames
  // lost every two seconds after the tenth. The first five seconds that hold
  // a lost one end at the twelfth: 80 % drawn.
  assert.equal(
    play({
      fps: 30,
      seconds: 30,
      drawn: (index) => index < 300 || Math.floor(index / 30) % 2 === 0,
    }),
    12,
  );
});

test("the first five seconds and a still screen tell nothing", { skip }, () => {
  // Nothing drawn at all, but only four seconds counted.
  assert.equal(play({ fps: 30, seconds: 4, drawn: () => false }), null);
  // A still screen: a frame every two seconds, none drawn.
  assert.equal(play({ fps: 0.5, seconds: 30, drawn: () => false }), null);
});

test("a new session starts the count over", { skip }, () => {
  const dog = new watchdog.UpscaleWatchdog(0);
  // Four seconds of frames, a second of them lost when the session ended.
  assert.equal(
    play({ fps: 30, seconds: 4, drawn: (index) => index < 90, dog }),
    null,
  );
  dog.reset(4000);
  assert.equal(play({ fps: 30, seconds: 4, start: 4000, dog }), null);
  // Without the reset the lost second would have counted.
  const kept = new watchdog.UpscaleWatchdog(0);
  play({ fps: 30, seconds: 4, drawn: (index) => index < 90, dog: kept });
  for (let index = 0; index < 30; index += 1) {
    kept.receive(4000 + index * 33);
  }
  assert.equal(kept.losing(5000.5), true);
});
