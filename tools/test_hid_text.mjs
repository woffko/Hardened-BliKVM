import assert from "node:assert/strict";
import test from "node:test";
import { textToKeyReports } from "../web/src/lib/hid-text.ts";

const SHIFT = 0x02;
const steps = (text, state) =>
  textToKeyReports(text, state ?? { script: "latin" }).map(
    ({ modifier, usage }) => [modifier, usage],
  );

test("Latin text uses US positions", () => {
  assert.deepEqual(steps("aZ1 "), [
    [0, 0x04],
    [SHIFT, 0x1d],
    [0, 0x1e],
    [0, 0x2c],
  ]);
});

test("Cyrillic text uses JCUKEN positions, including shifted keys", () => {
  // й=q ф=a я=z ё=` ; Х Ъ Ж Э Б Ю Ё need Shift on non-letter US keys.
  assert.deepEqual(steps("йфяё"), [
    [0, 0x14],
    [0, 0x04],
    [0, 0x1d],
    [0, 0x35],
  ]);
  assert.deepEqual(steps("ХЪЖЭБЮЁ"), [
    [SHIFT, 0x2f],
    [SHIFT, 0x30],
    [SHIFT, 0x33],
    [SHIFT, 0x34],
    [SHIFT, 0x36],
    [SHIFT, 0x37],
    [SHIFT, 0x35],
  ]);
  assert.deepEqual(steps("Привет"), [
    [SHIFT, 0x0a],
    [0, 0x0b],
    [0, 0x05],
    [0, 0x07],
    [0, 0x17],
    [0, 0x11],
  ]);
});

test("punctuation follows the script of the last letter", () => {
  // After Cyrillic, '.' is the '/' key and ',' is Shift+'/'.
  assert.deepEqual(steps("да."), [
    [0, 0x0f],
    [0, 0x09],
    [0, 0x38],
  ]);
  assert.deepEqual(steps("да,"), [
    [0, 0x0f],
    [0, 0x09],
    [SHIFT, 0x38],
  ]);
  // After Latin, '.' is the '.' key.
  assert.deepEqual(steps("ok."), [
    [0, 0x12],
    [0, 0x0e],
    [0, 0x37],
  ]);
  // The script carries across calls through the shared state.
  const state = { script: "latin" };
  textToKeyReports("мир", state);
  assert.equal(state.script, "cyrillic");
  assert.deepEqual(steps("?", state), [[SHIFT, 0x24]]);
  assert.deepEqual(steps(":", state), [[SHIFT, 0x23]]);
});

test("unsupported characters are skipped", () => {
  assert.deepEqual(steps("€"), []);
});

test("Estonian text uses the Estonian layout's keys, AltGr as the right Alt", () => {
  const ALTGR = 0x40;
  // ä on the US quote key; õ ö ü on ] ; [; š ž with AltGr on S and Z.
  assert.deepEqual(steps("tänan"), [
    [0, 0x17],
    [0, 0x34],
    [0, 0x11],
    [0, 0x04],
    [0, 0x11],
  ]);
  assert.deepEqual(steps("õöüšž"), [
    [0, 0x30],
    [0, 0x33],
    [0, 0x2f],
    [ALTGR, 0x16],
    [ALTGR, 0x1d],
  ]);
  assert.deepEqual(steps("ÕŠ"), [
    [SHIFT, 0x30],
    [SHIFT | ALTGR, 0x16],
  ]);
  // After an Estonian letter, Latin letters keep the layout and its
  // punctuation: '?' is Shift on the US '-' key, ';' Shift on ','.
  const state = { script: "latin" };
  assert.deepEqual(steps("ä ok?;", state), [
    [0, 0x34],
    [0, 0x2c],
    [0, 0x12],
    [0, 0x0e],
    [SHIFT, 0x2d],
    [SHIFT, 0x36],
  ]);
  assert.equal(state.script, "estonian");
  // A dead key is followed by a space; € is AltGr on 5.
  assert.deepEqual(steps("~€", state), [
    [SHIFT, 0x35],
    [0, 0x2c],
    [ALTGR, 0x22],
  ]);
  // Cyrillic leaves it.
  steps("да", state);
  assert.equal(state.script, "cyrillic");
});
