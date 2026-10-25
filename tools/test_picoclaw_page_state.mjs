// What the page keeps for the PicoClaw assistant (web/src/lib/uuid.ts,
// web/src/lib/picoclaw-storage.ts): an identifier from the browser's
// cryptographic generator even without crypto.randomUUID, and nothing of the
// assistant's left in the browser's storage after a logout (security review
// of 2026-10-18, LOW-05). The files are TypeScript, which Node reads by
// stripping its types; where this Node cannot, the tests are skipped.
import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

const readsTypeScript = Boolean(process.features.typescript);
const skip = readsTypeScript
  ? false
  : "this Node cannot read TypeScript (process.features.typescript)";
const load = (file) =>
  import(
    pathToFileURL(path.resolve(import.meta.dirname, "../web/src/lib", file))
      .href
  );
const uuid = readsTypeScript ? await load("uuid.ts") : null;
const storage = readsTypeScript ? await load("picoclaw-storage.ts") : null;

class MemoryStorage {
  values = new Map();
  getItem(key) {
    return this.values.has(key) ? this.values.get(key) : null;
  }
  setItem(key, value) {
    this.values.set(key, String(value));
  }
  removeItem(key) {
    this.values.delete(key);
  }
}

test(
  "without randomUUID the identifier comes from getRandomValues",
  { skip },
  () => {
    const calls = [];
    const fake = {
      getRandomValues(bytes) {
        calls.push(bytes.length);
        bytes.fill(0xab);
        return bytes;
      },
    };
    const id = uuid.generateUUIDv4(fake);
    assert.deepEqual(calls, [16]);
    assert.match(
      id,
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
  },
);

test("with randomUUID it is used", { skip }, () => {
  assert.equal(uuid.generateUUIDv4({ randomUUID: () => "given" }), "given");
});

test("a logout leaves none of the assistant's keys", { skip }, () => {
  globalThis.localStorage = new MemoryStorage();
  globalThis.window = { sessionStorage: new MemoryStorage() };
  storage.setPicoclawSessionId("session");
  storage.setPicoclawMaxSteps(10);
  storage.setPicoclawMaxRuntimeMs(5000);
  localStorage.setItem(
    "hardened-blikvm-picoclaw-runtime-install-snapshot",
    "{}",
  );
  localStorage.setItem("hardened-blikvm-language", "en");
  storage.clearPicoclawStorage();
  assert.deepEqual(
    [...localStorage.values.keys()],
    ["hardened-blikvm-language"],
  );
  assert.deepEqual([...window.sessionStorage.values.keys()], []);
});
