// The quality monitor's pure functions (web/src/lib/quality-monitor.ts,
// docs/design/h264-engine-choice-and-quality-monitor-v1.md): the page's rates
// from two samples, the rows of Basic and Extended for each path, and the
// window the browser keeps. The file is TypeScript, which Node reads by
// stripping its types; where this Node cannot, the tests are skipped and say
// so (docs/baselines/work-plan-20261004-review-fixes.md, B3).
import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

const readsTypeScript = Boolean(process.features.typescript);
const skip = readsTypeScript
  ? false
  : "this Node cannot read TypeScript (process.features.typescript)";
const monitor = readsTypeScript
  ? await import(
      pathToFileURL(
        path.resolve(import.meta.dirname, "../web/src/lib/quality-monitor.ts"),
      ).href
    )
  : null;

/** The page's texts that the rows put values into; the others show as keys. */
const TEXTS = {
  "qualityMonitor.h264": "H.264",
  "qualityMonitor.jpegQuality": "JPEG {{value}}",
  "qualityMonitor.videoEngine": "video engine",
  "qualityMonitor.chosen": "{{value}} chosen",
};
const t = (key, options) =>
  (TEXTS[key] ?? key).replace(/\{\{(\w+)\}\}/g, (_, name) =>
    String(options?.[name]),
  );
const valuesOf = (rows) =>
  Object.fromEntries(rows.map((row) => [row.key, row.value]));

const sample = (over = {}) => ({
  at: 0,
  bytes: null,
  framesReceived: null,
  framesDecoded: null,
  framesDrawn: null,
  framesDropped: null,
  delayMs: null,
  delayEstimated: false,
  decodeMs: null,
  decoder: null,
  width: null,
  height: null,
  quic: null,
  webrtc: null,
  ...over,
});
const encoder = (over = {}) => ({
  codec: "h264",
  streaming: true,
  engine: "hardware",
  engineWanted: "hardware",
  hardwareAvailable: true,
  hardwareReason: null,
  width: 1920,
  height: 1080,
  kbps: 8000,
  maxFps: 30,
  converters: 3,
  encodedFrames: 1000,
  stageMicros: { decode: 28100, convert: 9600, encode: 17600 },
  gate: { admitted: 1000, tooSoon: 500, unchanged: 20 },
  hardware: {
    passes: 1300,
    bothSearches: 200,
    wide: 150,
    higherQp: 9,
    leftOut: 3,
    keyFrames: 40,
    failures: 0,
    engineMicros: 9490000,
    passesPerSecond: 38,
    engineMicrosPerPass: 7300,
  },
  fps: 30,
  gatePerSecond: { admitted: 30, tooSoon: 15, unchanged: 0.5 },
  ...over,
});
const board = (over = {}) => ({
  capture: { width: 1920, height: 1080, fps: 30, queuedFps: 28, quality: 100 },
  h264: null,
  vp8: null,
  units: { h264CpuPercent: 4.5, vp8CpuPercent: null },
  temperatureMillidegrees: 68300,
  version: "0.1.0-test",
  ...over,
});
const rates = (over = {}) => ({
  megabitsPerSecond: 0.5,
  receivedPerSecond: 30,
  decodedPerSecond: 30,
  drawnPerSecond: 29.5,
  droppedPerSecond: 0,
  ...over,
});
const inputs = (over = {}) => ({
  path: "quic",
  codec: "h264",
  board: board({ h264: encoder() }),
  sample: sample({ delayMs: 46, width: 1920, height: 1080 }),
  rates: rates(),
  refreshHz: 60,
  ...over,
});

test("the page's rates come from two samples", { skip }, () => {
  const before = sample({
    at: 1000,
    bytes: 1000,
    framesReceived: 10,
    framesDecoded: 10,
    framesDrawn: 10,
    framesDropped: 0,
  });
  const after = sample({
    at: 2000,
    bytes: 126000,
    framesReceived: 40,
    framesDecoded: 40,
    framesDrawn: 39,
    framesDropped: 1,
  });
  assert.deepEqual(monitor.pageRates(before, after), {
    megabitsPerSecond: 1,
    receivedPerSecond: 30,
    decodedPerSecond: 30,
    drawnPerSecond: 29,
    droppedPerSecond: 1,
  });
  // Two seconds apart, half the rate.
  const later = sample({ ...after, at: 3000 });
  assert.equal(monitor.pageRates(before, later).receivedPerSecond, 15);
  // The first sample has nothing to be measured against.
  const none = monitor.pageRates(null, after);
  assert.ok(Object.values(none).every((value) => value === null));
});

test(
  "a rate is null where its count went back or its time did",
  { skip },
  () => {
    const before = sample({
      at: 5000,
      bytes: 9000,
      framesReceived: 90,
      framesDrawn: 5,
    });
    // The picture began again: the counts are smaller; the rest is still known.
    const reset = sample({
      at: 6000,
      bytes: 100,
      framesReceived: 3,
      framesDrawn: 9,
    });
    const rates = monitor.pageRates(before, reset);
    assert.equal(rates.megabitsPerSecond, null);
    assert.equal(rates.receivedPerSecond, null);
    assert.equal(rates.drawnPerSecond, 4);
    // A sample that is not later than the one before makes no rate at all.
    for (const at of [5000, 4000]) {
      const same = monitor.pageRates(before, sample({ ...reset, at }));
      assert.ok(
        Object.values(same).every((value) => value === null),
        `at ${at}`,
      );
    }
    // A count that one of them does not have.
    const rate = monitor.pageRates(
      sample({ at: 0 }),
      sample({ at: 1000, bytes: 5 }),
    );
    assert.equal(rate.megabitsPerSecond, null);
  },
);

test("Basic over QUIC: the numbers as shown", { skip }, () => {
  assert.deepEqual(valuesOf(monitor.basicRows(inputs(), t)), {
    speed: "0.500 Mbit/s",
    delay: "46 ms",
    path: "QUIC",
    fps: "29.5 / 30.0",
    bitrate: "8000 kbit/s",
    codec: "H.264 HW 1920x1080",
    chroma: "4:2:0",
  });
  const software = inputs({
    board: board({ h264: encoder({ engine: "x264" }) }),
  });
  assert.equal(
    valuesOf(monitor.basicRows(software, t)).codec,
    "H.264 SW 1920x1080",
  );
  // Without the board's answer: what the page knows, and dashes.
  const alone = valuesOf(monitor.basicRows(inputs({ board: null }), t));
  assert.equal(alone.fps, "29.5 / -");
  assert.equal(alone.bitrate, "-");
  assert.equal(alone.codec, "H.264 1920x1080");
});

test(
  "the Speed has three places below a megabit a second and two from it",
  { skip },
  () => {
    const speed = (megabitsPerSecond) =>
      valuesOf(
        monitor.basicRows(inputs({ rates: rates({ megabitsPerSecond }) }), t),
      ).speed;
    assert.equal(speed(0), "0.000 Mbit/s");
    assert.equal(speed(0.0176), "0.018 Mbit/s");
    assert.equal(speed(0.999), "0.999 Mbit/s");
    assert.equal(speed(1), "1.00 Mbit/s");
    assert.equal(speed(12.5), "12.50 Mbit/s");
    assert.equal(
      valuesOf(monitor.basicRows(inputs({ rates: null }), t)).speed,
      "-",
    );
  },
);

test("Basic over WebRTC marks the delay as an estimate", { skip }, () => {
  const webrtc = inputs({
    path: "webrtc",
    sample: sample({
      delayMs: 84,
      delayEstimated: true,
      width: 1280,
      height: 720,
    }),
  });
  const rows = valuesOf(monitor.basicRows(webrtc, t));
  assert.equal(rows.delay, "~84 ms");
  assert.equal(rows.path, "WebRTC");
  // The encoder's own size is the picture's size where the board knows it.
  assert.equal(rows.codec, "H.264 HW 1920x1080");
  const vp8 = inputs({
    path: "webrtc",
    codec: "vp8",
    board: board({
      vp8: encoder({ codec: "vp8", engine: "libvpx", kbps: 2500 }),
    }),
  });
  const vp8Rows = valuesOf(monitor.basicRows(vp8, t));
  assert.equal(vp8Rows.codec, "VP8 1920x1080");
  assert.equal(vp8Rows.bitrate, "2500 kbit/s");
});

test("Basic over MJPEG knows the capture only", { skip }, () => {
  const mjpeg = inputs({
    path: "mjpeg",
    codec: "mjpeg",
    sample: sample(),
    rates: rates({ megabitsPerSecond: 3 }),
  });
  assert.deepEqual(valuesOf(monitor.basicRows(mjpeg, t)), {
    speed: "-",
    delay: "-",
    path: "MJPEG",
    fps: "- / 30.0",
    bitrate: "JPEG 100",
    codec: "MJPEG 1920x1080",
    chroma: "4:2:2",
  });
});

test("Basic over Direct attributes no encoder to the page", { skip }, () => {
  // The Direct view is sent the capture device's own H.264: the encoder that
  // serves another viewer, and its engine and bitrate, are not this picture's.
  const direct = inputs({
    path: "direct",
    sample: sample({ bytes: 250000 }),
    rates: rates({
      megabitsPerSecond: 2,
      receivedPerSecond: null,
      decodedPerSecond: null,
      drawnPerSecond: null,
      droppedPerSecond: null,
    }),
  });
  const rows = valuesOf(monitor.basicRows(direct, t));
  assert.equal(rows.path, "H.264 Direct");
  assert.equal(rows.speed, "2.00 Mbit/s");
  assert.equal(rows.codec, "H.264 1920x1080");
  assert.equal(rows.bitrate, "-");
  // Nor does Extended show the encoder's rows.
  const board_ = monitor
    .extendedSections(direct, t)
    .find((section) => section.key === "board");
  assert.deepEqual(
    board_.rows.map((row) => row.key),
    ["version", "capture", "jpegQuality", "temperature"],
  );
});

test(
  "Extended has the sections of its path and leaves out the rest",
  { skip },
  () => {
    const quic = inputs({
      sample: sample({
        delayMs: 46,
        decoder: "WebCodecs",
        decodeMs: 3.2,
        quic: {
          roundTripMs: 3,
          packetsLost: 0,
          givenUp: 0,
          keyFrameRequests: 1,
          resets: 0,
          gatewayFramesSent: 100,
          gatewayFramesDropped: 0,
          gatewayKeyFrames: 5,
        },
      }),
    });
    const sections = monitor.extendedSections(quic, t);
    assert.deepEqual(
      sections.map((section) => section.key),
      ["basic", "transport", "board", "local"],
    );
    const byKey = Object.fromEntries(
      sections.map((section) => [
        section.key,
        section.rows.map((row) => row.key),
      ]),
    );
    assert.deepEqual(byKey.transport, [
      "quicRoundTrip",
      "quicLost",
      "quicFrames",
      "quicGivenUp",
      "quicRequests",
      "quicGateway",
    ]);
    assert.deepEqual(byKey.board, [
      "version",
      "capture",
      "jpegQuality",
      "encoder",
      "encoderFps",
      "stages",
      "enginePass",
      "engineCounts",
      "gate",
      "unitCpu",
      "temperature",
    ]);
    assert.deepEqual(byKey.local, [
      "decoder",
      "framesPerSecond",
      "decodeTime",
      "refresh",
    ]);
    const board_ = valuesOf(sections[2].rows);
    assert.equal(board_.stages, "28.1 / 9.6 / 17.6 ms");
    assert.equal(board_.temperature, "68.3 °C");
    assert.equal(board_.encoder, "video engine");
    assert.equal(board_.unitCpu, "5 %");
    // What the board chose, where it is not what encodes.
    const other = inputs({
      board: board({
        h264: encoder({ engine: "x264", engineWanted: "hardware" }),
      }),
    });
    const encoderRow = valuesOf(
      monitor
        .extendedSections(other, t)
        .find((section) => section.key === "board").rows,
    ).encoder;
    assert.equal(encoderRow, "x264 (video engine chosen)");

    const mjpeg = monitor.extendedSections(
      inputs({ path: "mjpeg", codec: "mjpeg", sample: sample() }),
      t,
    );
    const mjpegKeys = Object.fromEntries(
      mjpeg.map((section) => [section.key, section.rows.map((row) => row.key)]),
    );
    assert.deepEqual(mjpegKeys.transport, ["mjpegCapture", "mjpegQueued"]);
    assert.deepEqual(mjpegKeys.board, [
      "version",
      "capture",
      "jpegQuality",
      "temperature",
    ]);
    assert.deepEqual(mjpegKeys.local, ["refresh"]);

    const webrtc = monitor.extendedSections(
      inputs({
        path: "webrtc",
        sample: sample({
          webrtc: {
            roundTripMs: 4,
            candidates: "host - host",
            packetsReceived: 100,
            packetsLost: 3,
            jitterMs: 2,
            jitterBufferMsPerFrame: 50,
            nack: 1,
            pli: 2,
            fir: 0,
          },
        }),
      }),
      t,
    );
    assert.deepEqual(
      webrtc
        .find((section) => section.key === "transport")
        .rows.map((row) => row.key),
      [
        "rtcRoundTrip",
        "rtcCandidates",
        "rtcPackets",
        "rtcJitter",
        "rtcJitterBuffer",
        "rtcFeedback",
        "rtcFrames",
      ],
    );
  },
);

/** The browser's storage, as `loadLayout` and `saveLayout` find it. */
function storage(getItem, setItem = () => {}) {
  Object.defineProperty(globalThis, "localStorage", {
    value: { getItem, setItem },
    configurable: true,
  });
}

test(
  "the stored window is read within its bounds, or the first one is",
  { skip },
  () => {
    const first = monitor.DEFAULT_LAYOUT;
    storage(() => null);
    assert.deepEqual(monitor.loadLayout(), first);
    storage(() => "not json");
    assert.deepEqual(monitor.loadLayout(), first);
    storage(() => "[1,2]");
    assert.deepEqual(monitor.loadLayout(), { ...first, open: false });
    storage(() => {
      throw new Error("storage is blocked");
    });
    assert.deepEqual(monitor.loadLayout(), first);
    // Partial: the rest is the first window's.
    storage(() => '{"open":true,"level":"extended"}');
    assert.deepEqual(monitor.loadLayout(), {
      ...first,
      open: true,
      level: "extended",
    });
    // Anything but the exact values is the safe one.
    storage(() => '{"open":"yes","level":"huge","opaque":1}');
    assert.deepEqual(monitor.loadLayout(), first);
    // Numbers are kept inside what the page can show.
    storage(() => '{"x":1e9,"y":-5,"width":10,"height":1e9}');
    assert.deepEqual(monitor.loadLayout(), {
      ...first,
      x: 4000,
      y: 0,
      width: monitor.MIN_WIDTH,
      height: 4000,
    });
    storage(() => '{"x":"3","y":null,"width":"wide","height":NaN}');
    assert.deepEqual(monitor.loadLayout(), first);
  },
);

test(
  "the window is kept as it is, and a browser that keeps nothing is no error",
  { skip },
  () => {
    let kept = null;
    storage(
      () => kept,
      (_key, value) => {
        kept = value;
      },
    );
    const layout = {
      ...monitor.DEFAULT_LAYOUT,
      open: true,
      level: "extended",
      x: 40,
      y: 80,
    };
    monitor.saveLayout(layout);
    assert.deepEqual(monitor.loadLayout(), layout);
    storage(
      () => null,
      () => {
        throw new Error("storage is full");
      },
    );
    assert.doesNotThrow(() => monitor.saveLayout(layout));
  },
);
