import assert from "node:assert/strict";
import test from "node:test";
import {
  DOUBLE_TAP_MS,
  StickyModifiers,
  keyReport,
  modifierBits,
} from "../web/src/lib/sticky-modifiers.ts";

const CTRL = 0x01;
const SHIFT = 0x02;
const ALT = 0x04;
const WIN = 0x08;

test("a tap presses a modifier until the next key", () => {
  const modifiers = new StickyModifiers();
  assert.equal(modifiers.tap("ControlLeft", 0), "pressed");
  assert.equal(modifiers.mode("ControlLeft"), "oneShot");
  assert.equal(modifiers.bits(), CTRL);
  assert.equal(modifiers.consumeOneShot(), true);
  assert.equal(modifiers.mode("ControlLeft"), "off");
  assert.equal(modifiers.bits(), 0);
  assert.equal(modifiers.consumeOneShot(), false);
});

test("a second tap in time locks, and a key leaves it pressed", () => {
  const modifiers = new StickyModifiers();
  modifiers.tap("AltLeft", 1000);
  assert.equal(modifiers.tap("AltLeft", 1000 + DOUBLE_TAP_MS), "locked");
  assert.equal(modifiers.mode("AltLeft"), "locked");
  assert.equal(modifiers.consumeOneShot(), false);
  assert.equal(modifiers.bits(), ALT);
});

test("a tap on a pressed modifier asks for a release and changes nothing yet", () => {
  const modifiers = new StickyModifiers();
  modifiers.tap("MetaLeft", 0);
  assert.equal(modifiers.tap("MetaLeft", DOUBLE_TAP_MS + 1), "releasing");
  assert.equal(modifiers.bits(), WIN, "still pressed while the release waits");
  assert.equal(modifiers.release("MetaLeft"), true);
  assert.equal(modifiers.bits(), 0);
  assert.equal(modifiers.release("MetaLeft"), false);
});

test("a double tap on a pressed modifier locks it without a release", () => {
  const modifiers = new StickyModifiers();
  modifiers.tap("MetaLeft", 0);
  assert.equal(modifiers.tap("MetaLeft", 5000), "releasing");
  assert.equal(modifiers.tap("MetaLeft", 5100), "locked");
  assert.equal(modifiers.bits(), WIN);
  // A locked modifier is released the same way: one tap, then the wait.
  assert.equal(modifiers.tap("MetaLeft", 9000), "releasing");
  assert.equal(modifiers.mode("MetaLeft"), "locked");
  assert.equal(modifiers.release("MetaLeft"), true);
});

test("a third quick tap does not count as another double tap", () => {
  const modifiers = new StickyModifiers();
  modifiers.tap("ShiftLeft", 0);
  assert.equal(modifiers.tap("ShiftLeft", 100), "locked");
  assert.equal(modifiers.tap("ShiftLeft", 200), "releasing");
  assert.equal(modifiers.tap("ShiftLeft", 300), "locked");
});

test("taps on different modifiers are not a double tap", () => {
  const modifiers = new StickyModifiers();
  assert.equal(modifiers.tap("ControlLeft", 0), "pressed");
  assert.equal(modifiers.tap("ShiftLeft", 50), "pressed");
  assert.equal(modifiers.tap("ControlLeft", 100), "releasing");
  assert.equal(modifiers.bits(), CTRL | SHIFT);
});

test("a key between two taps ends the double tap", () => {
  const modifiers = new StickyModifiers();
  modifiers.tap("ControlLeft", 0);
  modifiers.consumeOneShot();
  assert.equal(modifiers.tap("ControlLeft", 200), "pressed");
  assert.equal(modifiers.mode("ControlLeft"), "oneShot");
});

test("one-shot and locked modifiers together: a key releases only the one-shot ones", () => {
  const modifiers = new StickyModifiers();
  modifiers.tap("ControlLeft", 0);
  modifiers.tap("ControlLeft", 100);
  modifiers.tap("ShiftLeft", 1000);
  assert.equal(modifiers.bits(), CTRL | SHIFT);
  assert.deepEqual(modifiers.snapshot(), {
    ControlLeft: "locked",
    ShiftLeft: "oneShot",
  });
  assert.equal(modifiers.consumeOneShot(), true);
  assert.equal(modifiers.bits(), CTRL);
  assert.equal(modifiers.reset(), true);
  assert.equal(modifiers.bits(), 0);
  assert.equal(modifiers.reset(), false);
  assert.deepEqual(modifiers.snapshot(), {});
});

test("reports carry the modifier byte and at most one key", () => {
  assert.deepEqual(keyReport(0), [0, 0, 0, 0, 0, 0, 0, 0]);
  assert.deepEqual(keyReport(CTRL | ALT, 0x4c), [0x05, 0, 0x4c, 0, 0, 0, 0, 0]);
  assert.equal(modifierBits(["ControlLeft", "AltLeft"]), CTRL | ALT);
  assert.equal(modifierBits([]), 0);
});
