// The JPEG frames of an MJPEG stream as the upscaled MJPEG view cuts them
// (web/src/lib/mjpeg-parts.ts): parts that say their length, parts that end
// at the next boundary, any split of the bytes into chunks, when each was
// captured, and what is not a stream. The file is TypeScript, which Node reads by stripping its types;
// where this Node cannot, the tests are skipped and say so.
import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

const readsTypeScript = Boolean(process.features.typescript);
const skip = readsTypeScript
  ? false
  : "this Node cannot read TypeScript (process.features.typescript)";
const mjpeg = readsTypeScript
  ? await import(
      pathToFileURL(
        path.resolve(import.meta.dirname, "../web/src/lib/mjpeg-parts.ts"),
      ).href
    )
  : null;

const bytes = (text) => new TextEncoder().encode(text);
const join = (...pieces) => {
  const out = new Uint8Array(
    pieces.reduce((sum, piece) => sum + piece.length, 0),
  );
  let at = 0;
  for (const piece of pieces) {
    out.set(piece, at);
    at += piece.length;
  }
  return out;
};
/** A JPEG-like body: a start marker, bytes that look like a boundary, an end marker. */
const body = (seed, length) => {
  const out = new Uint8Array(length);
  for (let index = 0; index < length; index += 1)
    out[index] = (seed * 31 + index * 7) & 0xff;
  out.set([0xff, 0xd8], 0);
  if (length > 20) out.set(bytes("--frame"), Math.floor(length / 2));
  out.set([0xff, 0xd9], length - 2);
  return out;
};
const withLength = (part, seconds = "1791529729.869000") =>
  join(
    bytes(
      `--frame\r\nContent-Type: image/jpeg\r\nContent-Length: ${part.length}\r\nX-Timestamp: ${seconds}\r\n\r\n`,
    ),
    part,
    bytes("\r\n"),
  );
const withoutLength = (part) =>
  join(
    bytes("--frame\r\nContent-Type: image/jpeg\r\n\r\n"),
    part,
    bytes("\r\n"),
  );

/** Feeds `stream` in chunks of `size` bytes; the parts in order. */
function cut(stream, size, boundary = "frame") {
  const parts = new mjpeg.MjpegParts(boundary);
  const found = [];
  for (let at = 0; at < stream.length; at += size) {
    found.push(...parts.push(stream.subarray(at, at + size)));
  }
  return found;
}

test("the boundary is read from the content type", { skip }, () => {
  assert.equal(
    mjpeg.boundaryOf("multipart/x-mixed-replace; boundary=frame"),
    "frame",
  );
  assert.equal(
    mjpeg.boundaryOf('multipart/x-mixed-replace;boundary="boundarydonotcross"'),
    "boundarydonotcross",
  );
  assert.equal(mjpeg.boundaryOf("image/jpeg"), null);
  assert.equal(mjpeg.boundaryOf(null), null);
});

test(
  "parts that say their length come out whole, whatever the chunks",
  { skip },
  () => {
    const parts = [body(1, 5000), body(2, 3), body(3, 70000)];
    const stream = join(...parts.map(withLength));
    for (const size of [1, 7, 4096, stream.length]) {
      const found = cut(stream, size);
      assert.equal(found.length, 3, `chunks of ${size}`);
      found.forEach((part, index) => assert.deepEqual(part.jpeg, parts[index]));
    }
  },
);

test("parts without a length end at the next boundary", { skip }, () => {
  const parts = [
    bytes("\xff\xd8first\xff\xd9"),
    bytes("\xff\xd8second\xff\xd9"),
  ];
  const stream = join(...parts.map(withoutLength), bytes("--frame\r\n"));
  for (const size of [1, 5, stream.length]) {
    const found = cut(stream, size);
    assert.equal(found.length, 2, `chunks of ${size}`);
    found.forEach((part, index) => {
      assert.deepEqual(part.jpeg, parts[index]);
      assert.equal(part.capturedMs, null);
    });
  }
});

test(
  "each part says when it was captured, as the board's streamer says it",
  { skip },
  () => {
    const stream = join(
      withLength(body(1, 100), "1791529729.869000"),
      withLength(body(2, 100), "1791529729.889500"),
    );
    const found = cut(stream, 7);
    assert.equal(found.length, 2);
    assert.equal(found[0].capturedMs, 1791529729869);
    assert.ok(
      Math.abs(found[1].capturedMs - found[0].capturedMs - 20.5) < 0.01,
    );
  },
);

test("what is not a stream, or a part too large, is refused", { skip }, () => {
  const parts = new mjpeg.MjpegParts("frame");
  assert.throws(
    () => parts.push(bytes(`--frame\r\n${"X".repeat(5000)}`)),
    /not a multipart stream/,
  );
  const small = new mjpeg.MjpegParts("frame", 1000);
  assert.throws(
    () => small.push(bytes("--frame\r\nContent-Length: 5000\r\n\r\n")),
    /part too large/,
  );
});
