import assert from "node:assert/strict";
import test from "node:test";

import { codecOf, decoderConfig } from "../web/src/lib/quic/h264.ts";

test("the codec string comes from the key frame's parameter set", () => {
  // x264, constrained baseline, level 4.0: a parameter set, a picture
  // parameter set and the start of a slice.
  const key = Uint8Array.from([
    0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x28, 0xd9, 0x00, 0x78, 0x02, 0x27, 0xe5, 0,
    0, 0, 1, 0x68, 0xcb, 0x83, 0xcb, 0x20, 0, 0, 1, 0x65, 0x88, 0x84, 0x00,
  ]);
  assert.equal(codecOf(key), "avc1.42c028");
  // Three-byte start codes, and something before the parameter set.
  assert.equal(
    codecOf(
      Uint8Array.from([
        0, 0, 1, 0x06, 0x05, 0xff, 0, 0, 1, 0x27, 0x64, 0x00, 0x1f, 0xac,
      ]),
    ),
    "avc1.64001f",
  );
  // A delta frame has none; nor has what is not a frame.
  assert.equal(
    codecOf(Uint8Array.from([0, 0, 0, 1, 0x41, 0x9a, 0x24, 0x6c, 0x41, 0x4f])),
    null,
  );
  assert.equal(codecOf(new Uint8Array(0)), null);
  assert.equal(codecOf(Uint8Array.from([0, 0, 1, 0x67, 0x42])), null);
  assert.equal(
    codecOf(Uint8Array.from({ length: 300 }, (_, index) => index & 0xff)),
    null,
  );
});

test("the decoder is told what the board's samples mean", () => {
  // The capture's JPEG YCbCr, full range with the BT.601 matrix, of an sRGB
  // picture: what Chromium calls JPEG's colour space. An encoder of an older
  // system does not say so in its stream, and the page draws it the same.
  assert.deepEqual(decoderConfig("avc1.42c028"), {
    codec: "avc1.42c028",
    optimizeForLatency: true,
    colorSpace: {
      primaries: "bt709",
      transfer: "iec61966-2-1",
      matrix: "smpte170m",
      fullRange: true,
    },
  });
});
