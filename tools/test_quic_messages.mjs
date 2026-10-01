import assert from "node:assert/strict";
import test from "node:test";
import {
  MAX_MESSAGE_BYTES,
  MessageReader,
  encodeMessage,
  hashBytes,
  pagePort,
} from "../web/src/lib/quic/messages.ts";

const text = (bytes) => new TextDecoder().decode(bytes);

test("a message is its length in two bytes and a JSON object, as the gateway reads it", () => {
  const hello = { t: "hello", protocol: 1, token: "ab".repeat(32) };
  const bytes = encodeMessage(hello);
  assert.equal((bytes[0] << 8) | bytes[1], bytes.length - 2);
  // The same bytes the gateway's own encoder makes (page.rs test).
  assert.ok(text(bytes.subarray(2)).startsWith('{"t":"hello","protocol":1,"token":"abab'));
  assert.equal(
    text(encodeMessage({ t: "keyframe", stream: 7, lastFrame: 41, dropped: 2 }).subarray(2)),
    '{"t":"keyframe","stream":7,"lastFrame":41,"dropped":2}',
  );
});

test("the reader returns messages as they complete, whatever the chunks", () => {
  const all = new Uint8Array([
    ...encodeMessage({ t: "welcome", protocol: 1, maxDatagram: 1200 }),
    ...encodeMessage({ t: "pong", id: 3, pageTime: 10.5, boardTime: 99 }),
    ...encodeMessage({ t: "closing", reason: "session" }),
  ]);
  for (const chunk of [1, 2, 3, 7, all.length]) {
    const reader = new MessageReader();
    const seen = [];
    for (let at = 0; at < all.length; at += chunk) {
      reader.push(all.subarray(at, at + chunk));
      for (let message = reader.take(); message; message = reader.take()) seen.push(message);
    }
    assert.deepEqual(seen, [
      { t: "welcome", protocol: 1, maxDatagram: 1200 },
      { t: "pong", id: 3, pageTime: 10.5, boardTime: 99 },
      { t: "closing", reason: "session" },
    ]);
    assert.equal(reader.pending, 0);
  }
});

test("a length out of range and a body that is no object end the stream", () => {
  const zero = new MessageReader();
  zero.push(new Uint8Array([0, 0]));
  assert.throws(() => zero.take(), RangeError);
  const huge = new MessageReader();
  huge.push(new Uint8Array([(MAX_MESSAGE_BYTES + 1) >> 8, (MAX_MESSAGE_BYTES + 1) & 0xff]));
  assert.throws(() => huge.take(), RangeError);
  for (const body of ["[1,2]", "7", "null", '"text"']) {
    const reader = new MessageReader();
    reader.push(new Uint8Array([0, body.length, ...new TextEncoder().encode(body)]));
    assert.throws(() => reader.take(), TypeError, body);
  }
  const garbage = new MessageReader();
  garbage.push(new Uint8Array([0, 3, 0x7b, 0xff, 0x7d]));
  assert.throws(() => garbage.take());
  assert.throws(() => encodeMessage({ t: "hello", token: "a".repeat(MAX_MESSAGE_BYTES) }), RangeError);
});

test("a certificate hash is 32 bytes from 64 lower-case digits", () => {
  const bytes = hashBytes("00ff".repeat(16));
  assert.equal(bytes.length, 32);
  assert.deepEqual([...bytes.subarray(0, 4)], [0, 255, 0, 255]);
  for (const bad of ["", "00ff", "00FF".repeat(16), "zz".repeat(32), "0".repeat(65)]) {
    assert.throws(() => hashBytes(bad), TypeError, bad);
  }
});

test("the page's port is the one in its address or its scheme's own", () => {
  assert.equal(pagePort({ port: "", protocol: "https:" }), 443);
  assert.equal(pagePort({ port: "", protocol: "http:" }), 80);
  assert.equal(pagePort({ port: "8443", protocol: "https:" }), 8443);
});
