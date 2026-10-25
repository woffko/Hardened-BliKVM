import assert from "node:assert/strict";
import test from "node:test";

import {
  HEADER_BYTES,
  MAX_DATAGRAM_BYTES,
  MAX_FRAME_BYTES,
  decodeHeader,
} from "../web/src/lib/quic/frame.ts";
import {
  HOLD_MS,
  LIFETIME_MS,
  MAX_ASSEMBLING_BYTES,
  MAX_ASSEMBLING_FRAMES,
  MAX_PENDING_BYTES,
  MAX_PENDING_FRAMES,
  PROGRESS_TIMEOUT_MS,
  VideoReceiver,
} from "../web/src/lib/quic/video-receiver.ts";

const STREAM = 7;

function header({
  key = false,
  index = 0,
  count = 1,
  stream = STREAM,
  frame,
  reference,
  size,
  capture = 5_000_000,
}) {
  const bytes = new Uint8Array(HEADER_BYTES);
  const view = new DataView(bytes.buffer);
  bytes[0] = 1;
  bytes[1] = key ? 1 : 2;
  bytes[2] = 1;
  view.setUint16(4, index);
  view.setUint16(6, count);
  view.setUint32(8, stream);
  view.setUint32(12, frame);
  view.setUint32(16, key ? frame : reference);
  view.setUint32(20, size);
  view.setUint32(24, Math.floor(capture / 0x100000000));
  view.setUint32(28, capture % 0x100000000);
  return bytes;
}

function join(...parts) {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

// A frame's bytes: recognisable, and different for every frame.
function payload(frame, size) {
  return Uint8Array.from(
    { length: size },
    (_, index) => (frame * 31 + index) & 0xff,
  );
}

// The datagrams of a delta frame, as the gateway cuts them.
function fragments(
  frame,
  reference,
  size,
  { capacity = 1168, stream = STREAM, capture } = {},
) {
  const data = payload(frame, size);
  const count = Math.ceil(size / capacity);
  return Array.from({ length: count }, (_, index) =>
    join(
      header({ index, count, stream, frame, reference, size, capture }),
      data.subarray(index * capacity, (index + 1) * capacity),
    ),
  );
}

function keyFrame(frame, size, { stream = STREAM, capture } = {}) {
  return join(
    header({ key: true, stream, frame, size, capture }),
    payload(frame, size),
  );
}

const ids = (output) => output.frames.map((frame) => frame.frame);

test("the header has the gateway's layout, and what cannot be is refused", () => {
  const bytes = header({
    index: 2,
    count: 9,
    stream: 0x01020304,
    frame: 0x0a0b0c0d,
    reference: 0x0a0b0c00,
    size: 10_000,
    capture: 0x0011223344556677 % 2 ** 53,
  });
  assert.deepEqual(
    Array.from(bytes.subarray(0, 24)),
    [
      1, 2, 1, 0, 0, 2, 0, 9, 1, 2, 3, 4, 0x0a, 0x0b, 0x0c, 0x0d, 0x0a, 0x0b,
      0x0c, 0, 0, 0, 0x27, 0x10,
    ],
  );
  assert.deepEqual(decodeHeader(bytes), {
    key: false,
    fragmentIndex: 2,
    fragmentCount: 9,
    stream: 0x01020304,
    frame: 0x0a0b0c0d,
    reference: 0x0a0b0c00,
    size: 10_000,
    captureMicros: 0x0011223344556677 % 2 ** 53,
  });
  assert.equal(
    decodeHeader(header({ key: true, frame: 5, size: 100 })).reference,
    5,
  );
  const refused = (
    change,
    base = { frame: 9, reference: 4, size: 5000, count: 5 },
  ) => {
    const bad = header(base);
    change(bad, new DataView(bad.buffer));
    assert.equal(decodeHeader(bad), null);
  };
  assert.equal(decodeHeader(new Uint8Array(31)), null);
  refused((bad) => (bad[0] = 2)); // version
  refused((bad) => (bad[1] = 3)); // kind
  refused((bad) => (bad[2] = 2)); // codec
  refused((bad) => (bad[3] = 1)); // reserved
  refused((_, view) => view.setUint32(8, 0)); // no stream
  refused((_, view) => view.setUint32(12, 0)); // no frame
  refused((_, view) => view.setUint32(16, 0)); // no reference
  refused((_, view) => view.setUint32(16, 9)); // a delta frame that refers to itself
  refused((_, view) => view.setUint32(20, 0)); // no size
  refused((_, view) => view.setUint32(20, MAX_FRAME_BYTES + 1));
  refused((_, view) => view.setUint16(6, 0)); // no fragments
  refused((_, view) => view.setUint16(4, 5)); // an index beyond the count
  refused((_, view) => view.setUint16(6, 5001)); // more fragments than bytes
  refused((_, view) => view.setUint32(24, 0x200000)); // a time no clock has reached
  refused((_, view) => view.setUint16(6, 2), {
    key: true,
    frame: 9,
    size: 100,
  });
  refused((_, view) => view.setUint16(4, 1), {
    key: true,
    frame: 9,
    size: 100,
  });
  refused((_, view) => view.setUint32(16, 8), {
    key: true,
    frame: 9,
    size: 100,
  });
});

test("a key frame and the frames after it are decoded in order", () => {
  const receiver = new VideoReceiver();
  const first = receiver.keyFrame(keyFrame(10, 50_000), 0);
  assert.deepEqual(ids(first), [10]);
  assert.equal(first.frames[0].key, true);
  assert.deepEqual(first.frames[0].data, payload(10, 50_000));
  assert.equal(first.request, null);
  let now = 10;
  for (let frame = 11; frame <= 20; frame += 1) {
    const size = 1000 * (frame - 9);
    const datagrams = fragments(frame, 10, size, { capture: frame * 1000 });
    datagrams.forEach((datagram, index) => {
      const output = receiver.datagram(datagram, now);
      now += 1;
      if (index < datagrams.length - 1)
        return assert.deepEqual(output.frames, []);
      assert.deepEqual(ids(output), [frame]);
      assert.equal(output.frames[0].key, false);
      assert.equal(output.frames[0].captureMicros, frame * 1000);
      assert.deepEqual(output.frames[0].data, payload(frame, size));
    });
  }
  assert.deepEqual(receiver.position(), { stream: STREAM, frame: 20 });
  assert.deepEqual(receiver.counters, {
    delivered: 11,
    dropped: 0,
    refused: 0,
    duplicates: 0,
    requests: 0,
  });
  assert.equal(receiver.waiting(), 0);
  assert.equal(receiver.waitingBytes(), 0);
});

test("fragments in any order, and what arrived before is ignored", () => {
  const receiver = new VideoReceiver();
  receiver.keyFrame(keyFrame(1, 1000), 0);
  const datagrams = fragments(2, 1, 9000);
  assert.equal(datagrams.length, 8);
  const order = [5, 0, 7, 3, 3, 1, 6, 0, 2, 4];
  const seen = [];
  order.forEach((index, step) =>
    seen.push(...ids(receiver.datagram(datagrams[index], step))),
  );
  assert.deepEqual(seen, [2]);
  assert.equal(receiver.counters.duplicates, 2);
  // After it was decoded: late copies change nothing.
  assert.deepEqual(ids(receiver.datagram(datagrams[7], 20)), []);
  assert.equal(receiver.waiting(), 0);
  assert.equal(receiver.counters.dropped, 0);
});

test("frames are held for the one before them and for their key frame", () => {
  const receiver = new VideoReceiver();
  // Delta frames of a key frame that has not arrived: they wait for it.
  for (const frame of [32, 31]) {
    for (const datagram of fragments(frame, 30, 3000)) {
      assert.deepEqual(ids(receiver.datagram(datagram, 0)), []);
    }
  }
  assert.equal(receiver.waiting(), 2);
  const first = receiver.keyFrame(keyFrame(30, 20_000), 5);
  assert.deepEqual(ids(first), [30, 31, 32]);
  // A key frame arrived when its stream was read; the others before it.
  assert.deepEqual(
    first.frames.map((frame) => frame.receivedAt),
    [5, 0, 0],
  );
  // 34 is whole before 33.
  for (const datagram of fragments(34, 30, 2000))
    receiver.datagram(datagram, 6);
  const later = fragments(33, 30, 2000);
  assert.deepEqual(ids(receiver.datagram(later[0], 7)), []);
  const both = receiver.datagram(later[1], 8);
  assert.deepEqual(ids(both), [33, 34]);
  // A frame began to arrive with its first fragment, whenever it was whole.
  assert.deepEqual(
    both.frames.map((frame) => frame.receivedAt),
    [7, 6],
  );

  // The next key frame is slower than the frames after it; frames of the
  // key frame before it still decode meanwhile.
  for (const datagram of fragments(37, 36, 1500))
    receiver.datagram(datagram, 9);
  assert.deepEqual(ids(receiver.datagram(fragments(35, 30, 900)[0], 10)), [35]);
  assert.deepEqual(ids(receiver.keyFrame(keyFrame(36, 30_000), 11)), [36, 37]);
  assert.equal(receiver.counters.requests, 0);
  assert.equal(receiver.counters.dropped, 0);
});

test("a fragment that contradicts its frame discards the frame", () => {
  const cases = {
    "another size": (bad, view) => view.setUint32(20, 4001),
    "another count": (bad, view) => view.setUint16(6, 5),
    "another reference": (bad, view) => view.setUint32(16, 2),
    "another capture time": (bad, view) => view.setUint32(28, 77),
    "other bytes in a fragment that arrived": (bad) =>
      (bad[HEADER_BYTES + 3] ^= 1),
  };
  for (const [what, change] of Object.entries(cases)) {
    const receiver = new VideoReceiver();
    receiver.keyFrame(keyFrame(3, 500), 0);
    const datagrams = fragments(4, 3, 4000);
    receiver.datagram(datagrams[0], 1);
    receiver.datagram(datagrams[1], 2);
    const bad = datagrams[what.startsWith("other bytes") ? 1 : 2].slice();
    change(bad, new DataView(bad.buffer));
    const output = receiver.datagram(bad, 3);
    assert.deepEqual(output.frames, [], what);
    assert.deepEqual(
      output.request,
      { stream: STREAM, lastFrame: 3, dropped: 1 },
      what,
    );
    assert.equal(receiver.waiting(), 0, what);
    // The rest of the frame, and the frames after it, are no use any more.
    assert.deepEqual(ids(receiver.datagram(datagrams[3], 4)), [], what);
    for (const datagram of fragments(5, 3, 800))
      assert.deepEqual(ids(receiver.datagram(datagram, 5)), [], what);
    assert.equal(receiver.waiting(), 0, what);
    assert.deepEqual(ids(receiver.keyFrame(keyFrame(6, 500), 6)), [6], what);
    assert.equal(receiver.recovering(), false, what);
  }

  // A fragment whose length does not fit the frame's cut.
  for (const [what, make] of Object.entries({
    "a short fragment in the middle": (d) => [
      d[0],
      d[1].subarray(0, d[1].length - 1),
    ],
    "a last fragment that is too long": (d) => [
      d[0],
      join(d[3], new Uint8Array(1)),
    ],
    "a last fragment that is too short": (d) => [
      d[0],
      d[3].subarray(0, d[3].length - 1),
    ],
    "a first fragment that does not make the count": (d) => [
      d[0].subarray(0, HEADER_BYTES + 500),
    ],
    "a fragment without bytes": (d) => [d[2].subarray(0, HEADER_BYTES)],
  })) {
    const receiver = new VideoReceiver();
    receiver.keyFrame(keyFrame(3, 500), 0);
    let requested = null;
    make(fragments(4, 3, 4000)).forEach((datagram, step) => {
      requested = receiver.datagram(datagram, step).request ?? requested;
    });
    assert.deepEqual(
      requested,
      { stream: STREAM, lastFrame: 3, dropped: 1 },
      what,
    );
    assert.equal(receiver.waiting(), 0, what);
  }
});

test("a frame that stops arriving is given up, and a key frame is asked for", () => {
  const receiver = new VideoReceiver();
  receiver.keyFrame(keyFrame(1, 1000), 0);
  const lost = fragments(2, 1, 5000);
  receiver.datagram(lost[0], 100);
  receiver.datagram(lost[1], 150);
  // The next frames arrive whole and wait for it.
  for (const datagram of fragments(3, 1, 900)) receiver.datagram(datagram, 160);
  for (const datagram of fragments(4, 1, 900)) receiver.datagram(datagram, 200);
  assert.equal(receiver.waiting(), 3);
  assert.equal(receiver.poll(150 + PROGRESS_TIMEOUT_MS - 1).request, null);
  const given = receiver.poll(150 + PROGRESS_TIMEOUT_MS);
  assert.deepEqual(given.request, { stream: STREAM, lastFrame: 1, dropped: 3 });
  assert.equal(receiver.waiting(), 0);
  assert.equal(receiver.counters.dropped, 3);
  // Its last fragments come after all: too late.
  for (const datagram of lost.slice(2))
    assert.deepEqual(ids(receiver.datagram(datagram, 300)), []);
  assert.equal(receiver.waiting(), 0);
  // The key frame that answers, and the frames after it.
  assert.deepEqual(ids(receiver.keyFrame(keyFrame(6, 8000), 400)), [6]);
  assert.deepEqual(ids(receiver.datagram(fragments(7, 6, 700)[0], 401)), [7]);
  assert.equal(receiver.poll(5000).request, null);
  assert.equal(receiver.counters.requests, 1);
});

test("a pause before a frame is not counted against it; a pause inside it is", () => {
  const receiver = new VideoReceiver();
  receiver.keyFrame(keyFrame(1, 1000), 0);
  assert.deepEqual(ids(receiver.datagram(fragments(2, 1, 500)[0], 10)), [2]);
  // The screen stood still for five seconds. The next frame has six
  // fragments, each 100 ms after the one before.
  const slow = fragments(3, 1, 6000);
  let now = 5000;
  let delivered = [];
  for (const datagram of slow) {
    const output = receiver.datagram(datagram, now);
    assert.equal(output.request, null);
    delivered = delivered.concat(ids(output));
    now += 100;
  }
  assert.deepEqual(delivered, [3]);

  // A frame that keeps arriving, but not within a second.
  const endless = fragments(4, 1, 14_000);
  assert.equal(endless.length, 12);
  let request = null;
  now = 10_000;
  for (const datagram of endless) {
    const output = receiver.datagram(datagram, now);
    request = output.request ?? request;
    assert.deepEqual(output.frames, []);
    now += 100;
  }
  assert.deepEqual(request, { stream: STREAM, lastFrame: 3, dropped: 1 });
  assert.equal(now - 10_000 > LIFETIME_MS, true);
});

test("one request at a time: at once, after 1, 2 and 4 s, then none for 10 s", () => {
  const receiver = new VideoReceiver();
  // Nothing was ever decoded: a frame that cannot be is reason enough.
  const start = 1000;
  receiver.datagram(fragments(9, 8, 3000)[0], start - PROGRESS_TIMEOUT_MS);
  const times = [];
  const note = (output, now) => {
    if (!output.request) return;
    times.push(now - start);
    assert.equal(output.request.stream, 0);
    assert.equal(output.request.lastFrame, 0);
  };
  for (let now = start; now <= start + 40_000; now += 50) {
    // More loss meanwhile asks for nothing more.
    if (now % 1000 === 0)
      note(receiver.datagram(fragments(now, now - 1, 3000)[0], now), now);
    note(receiver.poll(now), now);
  }
  assert.deepEqual(
    times,
    [
      0, 1000, 3000, 7000, 17_000, 18_000, 20_000, 24_000, 34_000, 35_000,
      37_000,
    ],
  );
  assert.equal(receiver.counters.requests, times.length);
  // A key frame ends it, at whatever point of the schedule.
  receiver.keyFrame(keyFrame(50_000, 100), start + 40_100);
  assert.equal(receiver.poll(start + 60_000).request, null);
});

test("a decoder that fails is a broken chain", () => {
  const receiver = new VideoReceiver();
  receiver.keyFrame(keyFrame(1, 1000), 0);
  assert.deepEqual(receiver.decoderFailed(5).request, {
    stream: STREAM,
    lastFrame: 1,
    dropped: 0,
  });
  assert.deepEqual(ids(receiver.datagram(fragments(2, 1, 500)[0], 6)), []);
  assert.deepEqual(ids(receiver.keyFrame(keyFrame(3, 1000), 7)), [3]);
  // Before anything was decoded.
  const fresh = new VideoReceiver();
  assert.deepEqual(fresh.decoderFailed(0).request, {
    stream: 0,
    lastFrame: 0,
    dropped: 0,
  });
});

test("what waits is bounded: frames being put together, and all that wait", () => {
  // Frames of a key frame to come, being put together (one fragment of
  // three each): the fifth takes the place of the oldest.
  const receiver = new VideoReceiver();
  receiver.keyFrame(keyFrame(1, 1000), 0);
  for (let frame = 11; frame <= 15; frame += 1) {
    receiver.datagram(fragments(frame, 10, 3000)[0], frame);
  }
  assert.equal(receiver.assembling(), MAX_ASSEMBLING_FRAMES);
  assert.equal(receiver.waiting(), MAX_ASSEMBLING_FRAMES);
  assert.equal(receiver.counters.dropped, 1);
  assert.equal(receiver.waitingBytes(), 4 * 3000);

  // Complete frames of a key frame to come wait, up to 120 of them; the
  // next pushes the oldest out, and a key frame is asked for.
  const held = new VideoReceiver();
  held.keyFrame(keyFrame(1, 1000), 0);
  for (let frame = 11; frame < 11 + MAX_PENDING_FRAMES; frame += 1) {
    for (const datagram of fragments(frame, 10, 1500)) {
      held.datagram(datagram, frame);
    }
  }
  assert.equal(held.waiting(), MAX_PENDING_FRAMES);
  assert.equal(held.assembling(), 0);
  assert.equal(held.counters.dropped, 0);
  assert.equal(held.recovering(), false);
  for (const datagram of fragments(11 + MAX_PENDING_FRAMES, 10, 1500)) {
    held.datagram(datagram, 200);
  }
  assert.equal(held.waiting(), MAX_PENDING_FRAMES);
  assert.equal(held.counters.dropped, 1);
  assert.equal(held.recovering(), true);

  // Bytes being put together: two frames of 700 KiB fit, a third pushes
  // the oldest out.
  const large = new VideoReceiver();
  const size = 700 * 1024;
  for (let frame = 21; frame <= 24; frame += 1) {
    large.datagram(fragments(frame, 20, size)[0], frame);
    assert.ok(large.waitingBytes() <= MAX_ASSEMBLING_BYTES);
  }
  assert.equal(large.assembling(), 2);
  // A delta frame that could never be put together is refused.
  const huge = new VideoReceiver();
  huge.datagram(fragments(31, 30, MAX_ASSEMBLING_BYTES + 1)[0], 0);
  assert.equal(huge.waiting(), 0);
  assert.equal(huge.counters.refused, 1);
  assert.equal(huge.recovering(), true);
});

test("a key frame slower than a hundred of its frames: they wait for it and follow it in order", () => {
  const receiver = new VideoReceiver();
  receiver.keyFrame(keyFrame(1, 1000), 0);
  // The key frame 30 is on its way on its stream; its delta frames come
  // complete before it, 15 ms apart, for 1.5 s (within the 2 s they wait).
  for (let frame = 31; frame <= 130; frame += 1) {
    for (const datagram of fragments(frame, 30, 4000)) {
      assert.deepEqual(ids(receiver.datagram(datagram, frame * 15)), []);
    }
    receiver.poll(frame * 15 + 1);
  }
  assert.equal(receiver.waiting(), 100);
  const released = receiver.keyFrame(keyFrame(30, 400_000), 130 * 15 + 5);
  assert.deepEqual(
    ids(released),
    Array.from({ length: 101 }, (_, index) => 30 + index),
  );
  assert.equal(receiver.counters.dropped, 0);
  assert.equal(receiver.counters.requests, 0);
  assert.equal(receiver.waiting(), 0);
});

test("a key frame that does not come within 2 s: its frames are given up and it is asked for", () => {
  const receiver = new VideoReceiver();
  receiver.keyFrame(keyFrame(1, 1000), 0);
  for (const frame of [31, 32]) {
    for (const datagram of fragments(frame, 30, 2000)) {
      receiver.datagram(datagram, 100);
    }
  }
  assert.equal(receiver.poll(100 + HOLD_MS - 1).request, null);
  assert.equal(receiver.waiting(), 2);
  const given = receiver.poll(100 + HOLD_MS);
  assert.equal(receiver.waiting(), 0);
  assert.equal(receiver.counters.dropped, 2);
  assert.notEqual(given.request, null);
});

test("old frames, old key frames and another stream", () => {
  const receiver = new VideoReceiver();
  receiver.keyFrame(keyFrame(10, 1000), 0);
  for (let frame = 11; frame <= 13; frame += 1)
    receiver.datagram(fragments(frame, 10, 600)[0], frame);
  // A key frame that was overtaken, and a frame of the key frame before.
  assert.deepEqual(ids(receiver.keyFrame(keyFrame(9, 1000), 20)), []);
  assert.deepEqual(ids(receiver.datagram(fragments(14, 5, 600)[0], 21)), []);
  assert.deepEqual(receiver.position(), { stream: STREAM, frame: 13 });
  assert.equal(receiver.waiting(), 0);

  // The encoder started anew: its frames wait for its key frame, and what
  // waited of the old stream goes.
  receiver.datagram(fragments(15, 10, 5000)[0], 30);
  assert.deepEqual(
    ids(receiver.datagram(fragments(2, 1, 600, { stream: 8 })[0], 31)),
    [],
  );
  const restarted = receiver.keyFrame(keyFrame(1, 2000, { stream: 8 }), 32);
  assert.deepEqual(
    restarted.frames.map((frame) => [frame.stream, frame.frame]),
    [
      [8, 1],
      [8, 2],
    ],
  );
  assert.deepEqual(receiver.position(), { stream: 8, frame: 2 });
  assert.equal(receiver.waiting(), 0);
  // What is not a datagram of a delta frame, or a whole key frame.
  const refused = receiver.counters.refused;
  receiver.datagram(keyFrame(9, 100, { stream: 8 }), 40);
  receiver.datagram(new Uint8Array(MAX_DATAGRAM_BYTES + 1), 40);
  receiver.keyFrame(fragments(3, 1, 600, { stream: 8 })[0], 40);
  receiver.keyFrame(keyFrame(9, 100, { stream: 8 }).subarray(0, 100), 40);
  receiver.keyFrame(
    join(keyFrame(9, 100, { stream: 8 }), new Uint8Array(1)),
    40,
  );
  assert.equal(receiver.counters.refused, refused + 5);
  assert.deepEqual(receiver.position(), { stream: 8, frame: 2 });
});

// mulberry32
function generator(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let value = state;
    value = Math.imul(value ^ (value >>> 15), value | 1);
    value ^= value + Math.imul(value ^ (value >>> 7), value | 61);
    return ((value ^ (value >>> 14)) >>> 0) / 4294967296;
  };
}

test("100000 random and damaged datagrams and streams: bounded, and never a frame out of its chain", () => {
  const random = generator(20261002);
  const pick = (limit) => Math.floor(random() * limit);
  const receiver = new VideoReceiver();
  let now = 0;
  let stream = 1;
  let key = 0;
  let frame = 0;
  let last = null;
  let cases = 0;
  const check = (output) => {
    for (const delivered of output.frames) {
      if (delivered.key) {
        if (last && last.stream === delivered.stream)
          assert.ok(delivered.frame > last.frame);
      } else {
        assert.ok(last, "a delta frame before any key frame");
        assert.equal(delivered.stream, last.stream);
        assert.equal(delivered.frame, last.frame + 1);
      }
      assert.ok(delivered.data.length > 0);
      last = { stream: delivered.stream, frame: delivered.frame };
    }
    assert.ok(receiver.waiting() <= MAX_PENDING_FRAMES);
    assert.ok(receiver.assembling() <= MAX_ASSEMBLING_FRAMES);
    assert.ok(receiver.waitingBytes() <= MAX_PENDING_BYTES);
    assert.ok(receiver.waitingBytes() >= 0);
    cases += 1;
  };
  const damage = (bytes) => {
    const out = bytes.slice();
    switch (pick(4)) {
      case 0: // a byte of the header
        out[pick(HEADER_BYTES)] = pick(256);
        return out;
      case 1: // cut short
        return out.subarray(0, pick(out.length));
      case 2: // a byte anywhere
        out[pick(out.length)] ^= 1 << pick(8);
        return out;
      default: // something after it
        return join(out, new Uint8Array(1 + pick(40)));
    }
  };
  while (cases < 100_000) {
    now += pick(40);
    const what = pick(100);
    if (what < 6) {
      // A key frame, now and then of a new stream.
      if (pick(20) === 0) {
        stream += 1;
        frame = 0;
      }
      frame += 1;
      key = frame;
      let bytes = keyFrame(frame, 200 + pick(3000), { stream });
      if (pick(4) === 0) bytes = damage(bytes);
      if (pick(5) !== 0) check(receiver.keyFrame(bytes, now));
    } else if (what < 80 && key > 0) {
      // A delta frame: some fragments lost, repeated, out of order, damaged.
      frame += 1;
      const datagrams = fragments(frame, key, 100 + pick(6000), {
        stream,
        capacity: 200 + pick(969),
      });
      for (let index = datagrams.length - 1; index > 0; index -= 1) {
        if (pick(4) === 0) {
          const other = pick(index + 1);
          [datagrams[index], datagrams[other]] = [
            datagrams[other],
            datagrams[index],
          ];
        }
      }
      for (const datagram of datagrams) {
        if (pick(50) === 0) continue;
        const bytes = pick(60) === 0 ? damage(datagram) : datagram;
        check(receiver.datagram(bytes, now));
        if (pick(30) === 0) check(receiver.datagram(datagram, now));
        if (pick(25) === 0) now += pick(200);
      }
    } else if (what < 90) {
      // Bytes that are nothing.
      const bytes = Uint8Array.from({ length: pick(1400) }, () => pick(256));
      check(
        pick(2) === 0
          ? receiver.datagram(bytes, now)
          : receiver.keyFrame(bytes, now),
      );
    } else if (what < 97) {
      now += pick(1500);
      check(receiver.poll(now));
    } else {
      check(receiver.decoderFailed(now));
    }
  }
  // It was not all refused: frames were decoded, lost and asked for.
  assert.ok(
    receiver.counters.delivered > 2000,
    `${receiver.counters.delivered} delivered`,
  );
  assert.ok(
    receiver.counters.dropped > 500,
    `${receiver.counters.dropped} dropped`,
  );
  assert.ok(
    receiver.counters.requests > 50,
    `${receiver.counters.requests} requests`,
  );
  assert.ok(
    receiver.counters.refused > 500,
    `${receiver.counters.refused} refused`,
  );
});

// A group's stream (protocol 2): a key frame, then whole delta frames.
const groupDelta = (frame, reference, size) =>
  join(
    header({ index: 0, count: 1, frame, reference, size }),
    payload(frame, size),
  );

test("a group's stream is cut into its frames however its bytes come", async () => {
  const { GroupRecords } = await import("../web/src/lib/quic/group.ts");
  const stream = join(
    keyFrame(10, 5000),
    groupDelta(11, 10, 1),
    groupDelta(12, 10, 70_000),
    groupDelta(13, 10, 3),
  );
  for (const size of [1, 7, 4096, stream.length]) {
    const records = new GroupRecords();
    const found = [];
    for (let at = 0; at < stream.length; at += size) {
      found.push(...records.push(stream.subarray(at, at + size)));
    }
    assert.equal(found.length, 4, `chunks of ${size}`);
    assert.deepEqual(found[0], keyFrame(10, 5000));
    assert.deepEqual(found[2], groupDelta(12, 10, 70_000));
  }
  // A group begins with its key frame, and has no other.
  assert.throws(
    () => new GroupRecords().push(groupDelta(11, 10, 100)),
    /not a group/,
  );
  const twice = new GroupRecords();
  twice.push(keyFrame(10, 100));
  assert.throws(() => twice.push(keyFrame(20, 100)), /not a group/);
  // A delta frame in fragments is not a group's.
  const cut = new GroupRecords();
  cut.push(keyFrame(10, 100));
  assert.throws(
    () => cut.push(fragments(11, 10, 3000)[0]),
    /delta frame in fragments/,
  );
  assert.throws(
    () => new GroupRecords().push(new Uint8Array(40)),
    /not a frame header/,
  );
  // A frame larger than any is refused by its header, before its bytes.
  assert.throws(
    () =>
      new GroupRecords().push(
        header({ key: true, frame: 10, size: 4 * 1024 * 1024 + 1 }),
      ),
    /not a frame header/,
  );
});

test("a group's delta frames follow its key frame, whatever their size", () => {
  const receiver = new VideoReceiver();
  assert.deepEqual(ids(receiver.keyFrame(keyFrame(10, 5000), 0)), [10]);
  for (const [frame, size] of [
    [11, 1],
    [12, 300_000],
    [13, 2],
  ]) {
    assert.deepEqual(
      ids(receiver.groupFrame(groupDelta(frame, 10, size), frame)),
      [frame],
    );
  }
  // What is not a whole delta frame of a group is refused.
  receiver.groupFrame(keyFrame(20, 100), 20);
  receiver.groupFrame(fragments(14, 10, 3000)[0], 21);
  receiver.groupFrame(groupDelta(14, 10, 100).subarray(0, 50), 22);
  assert.equal(receiver.counters.refused, 3);
  assert.equal(receiver.counters.requests, 0);
});
