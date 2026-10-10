/**
 * The picture of the QUIC transport, away from the page's own thread: it
 * puts the frames together (`video-receiver.ts`), decodes them with
 * WebCodecs and draws them on the canvas it was given. The page hands it
 * what the session delivers and sends on what it asks for.
 */
import { Spans } from '../busy.ts';
import { drawing, type Sharpness } from '../upscale/settings.ts';
import { Upscaler, type UpscaleReport } from '../upscale/upscaler.ts';
import { UpscaleWatchdog, type UpscaleStop } from '../upscale/watchdog.ts';
import { BoardClock } from './board-clock.ts';
import { codecOf, decoderConfig } from './h264.ts';
import { VideoReceiver, type ReceivedFrame, type ReceiverOutput } from './video-receiver.ts';

export type ToVideoWorker =
  /**
   * The canvas to draw on, and whether to upscale: with a sharpness the
   * worker draws through WebGL2 (docs/design/screen-upscaling-v1.md), `off`
   * with a 2D context as before. A canvas has one kind of context: the
   * page hands over a new canvas to change it.
   */
  | { type: 'canvas'; canvas: OffscreenCanvas; upscale: Sharpness | 'off' }
  /** The picture's size on the page, in CSS pixels, and the display's pixel ratio. */
  | { type: 'view'; width: number; height: number; pixelRatio: number }
  /** Another sharpness, while upscaling. */
  | { type: 'sharpness'; sharpness: Sharpness }
  /**
   * Whether the page is seen: a hidden page's picture is decoded (the chain
   * of frames goes on) but not drawn, and its newest frame is drawn when the
   * page is seen again.
   */
  | { type: 'visible'; visible: boolean }
  /** A new session or subscription: nothing received before it counts. */
  | { type: 'begin' }
  | { type: 'datagram'; data: ArrayBuffer }
  | { type: 'keyframe'; data: ArrayBuffer }
  /** A whole delta frame of a group's stream (protocol 2). */
  | { type: 'groupframe'; data: ArrayBuffer }
  /**
   * A ping answered: its round trip, the board's clock in the answer, and
   * when the answer arrived, as `performance.timeOrigin + performance.now()`
   * of the page (a time the worker can place on its own clock).
   */
  | { type: 'pong'; roundTripMs: number; boardMicros: number; at: number };

export type VideoStats = {
  /** Frames drawn since the picture began, and in the last second. */
  frames: number;
  framesPerSecond: number;
  /**
   * Frames a second over the last five seconds: a still screen gives a
   * frame every two seconds, and a second then holds one or none.
   */
  frameRate: number;
  /**
   * Capture to drawing by the board's clock, of the frames whose age became
   * known in the last five seconds: a frame's age is known once a ping
   * after it has been answered.
   */
  ageMedianMs: number | null;
  ageP95Ms: number | null;
  /**
   * From a frame's arrival (its first fragment, or its key frame's stream)
   * to its drawing, over the last five seconds: what the page itself adds.
   */
  waitMedianMs: number | null;
  /** Frames given up or impossible to decode, and key-frame requests made. */
  dropped: number;
  requests: number;
  /** Times the decoder was started again: after an error, or when it fell behind. */
  resets: number;
  /** Frames the decoder has not come to yet. */
  queued: number;
  /**
   * For the quality monitor: the bytes of every datagram and key frame
   * that came, the frames handed to the decoder, both since the picture
   * began, and how long the decoder took for a frame in the median over the
   * last five seconds.
   */
  bytes: number;
  received: number;
  decodeMedianMs: number | null;
  /** How the frames are drawn: the plain path, or through the upscaler, and its time. */
  upscale: UpscaleReport;
  /** The canvas's buffer: the frame's size, or the size it is shown at when upscaled. */
  canvasWidth: number;
  canvasHeight: number;
  /** The share of the last second this worker's own handlers took, from 0 to 1. */
  busy: number;
};

export type FromVideoWorker =
  | { type: 'request'; stream: number; lastFrame: number; dropped: number }
  | { type: 'picture'; width: number; height: number }
  /**
   * The upscaler cannot draw (no WebGL2 here, or its context was lost), or
   * frames were lost to it (the watchdog). The page goes on with the plain
   * path.
   */
  | { type: 'upscaler'; state: UpscaleStop }
  | ({ type: 'stats' } & VideoStats);

/** How often frames that wait are looked at, and a due request is made. */
const POLL_MS = 20;
const STATS_MS = 1000;
/** What a report says about the frames covers this much of the past. */
const WINDOW_MS = 5000;
/**
 * A decoder that has not come to a frame handed to it this long ago has
 * fallen behind: it is started again from a key frame rather than show a
 * picture that grows old. Frames that come together (those that waited for
 * their key frame, those after a pause on the path) are not that, however
 * many.
 */
const DECODE_LAG_MS = 1000;
/** Frames the decoder may have before it at most, whatever their age. */
const MAX_DECODE_QUEUE = 120;
/** Frames that may wait for a ping to tell their age. */
const MAX_UNAGED = 256;

const scope = self as unknown as {
  postMessage(message: FromVideoWorker): void;
  onmessage: ((event: MessageEvent<ToVideoWorker>) => void) | null;
};

const receiver = new VideoReceiver();
const clock = new BoardClock();
let canvas: OffscreenCanvas | null = null;
let context: OffscreenCanvasRenderingContext2D | null = null;
/** The upscaler, when the page asked for one and WebGL2 is here; its sharpness. */
let upscaler: Upscaler | null = null;
let sharpness: Sharpness = 'medium';
/** While upscaling: whether frames are lost to it, and whether the page was told. */
let watchdog: UpscaleWatchdog | null = null;
let watchdogTold = false;
/** Whether the page is seen, and the newest frame decoded while it was not. */
let visible = true;
let hiddenFrame: VideoFrame | null = null;
/** Where the picture is shown; until the page says, the frame's own size. */
let view: { width: number; height: number; pixelRatio: number } | null = null;
/** The frame's size the page was last told. */
let pictureWidth = 0;
let pictureHeight = 0;
/** How the last frame was drawn, and the draws' times in the last seconds. */
let drawnMode: UpscaleReport['mode'] = 'off';
let drawTimes: { at: number; ms: number }[] = [];
let gpuTimes: { at: number; ms: number }[] = [];
let decoder: VideoDecoder | null = null;
let configured: string | null = null;
let frames = 0;
let resets = 0;
let bytes = 0;
let handed = 0;
/** When each frame the decoder has was handed to it, by its capture time. */
const handedAt = new Map<number, number>();
/** How long the decoder took for the frames of the last seconds, and when. */
let decodeTimes: { at: number; ms: number }[] = [];
/** The frames of the last seconds: when they were drawn, and how long after they arrived. */
let drawn: { at: number; wait: number | null }[] = [];
/** When the frames the decoder has began to arrive, by their capture time. */
const received = new Map<number, number>();
/**
 * Frames drawn after the newest answered ping: when, and their capture time
 * on the board's clock. The board's time at their drawing is read between
 * two pings, so their age waits for the next one.
 */
let unaged: { at: number; capture: number }[] = [];
/** The ages that became known in the last seconds, and when. */
let aged: { at: number; age: number }[] = [];
/**
 * For a probe that wants numbers over a span of its own: it reads this from
 * the worker twice and takes the difference. Counted since the worker began.
 */
const AGE_BUCKETS = 5000;
const WAIT_BUCKETS = 10000;
const probe = {
  /** Frames drawn. */
  frames: 0,
  /** How old the frames were when drawn: a count for every millisecond, the last for all that were older. */
  ages: new Uint32Array(AGE_BUCKETS + 1),
  /** From arrival to drawing: a count for every tenth of a millisecond. */
  waits: new Uint32Array(WAIT_BUCKETS + 1),
  /** The capture time of the newest frame drawn, microseconds of the board's clock. */
  lastCaptureMicros: 0
};
(self as unknown as { quicVideoProbe: typeof probe }).quicVideoProbe = probe;

function closeDecoder() {
  const old = decoder;
  decoder = null;
  configured = null;
  received.clear();
  handedAt.clear();
  if (old && old.state !== 'closed') {
    try {
      old.close();
    } catch {
      // Closed by its own error.
    }
  }
}

/** Draws a frame through the upscaler, into a buffer of the size it is shown at. */
function upscaled(target: OffscreenCanvas, frame: VideoFrame, scaler: Upscaler) {
  const width = frame.displayWidth;
  const height = frame.displayHeight;
  const how = view
    ? drawing(width, height, view.width, view.height, view.pixelRatio, sharpness)
    : { mode: 'plain' as const, width, height };
  if (target.width !== how.width || target.height !== how.height) {
    target.width = how.width;
    target.height = how.height;
  }
  const started = performance.now();
  if (scaler.draw(frame, width, height, { mode: how.mode })) {
    const ended = performance.now();
    watchdog?.draw(ended);
    drawTimes.push({ at: ended, ms: ended - started });
    for (const ms of scaler.takeGpuTimes()) gpuTimes.push({ at: ended, ms });
    drawnMode = how.mode;
  }
}

function draw(frame: VideoFrame) {
  if (!visible) {
    // Kept, the newest only, to be drawn once the page is seen; its
    // bookkeeping is the decoder's (when it was handed over and came).
    hiddenFrame?.close();
    hiddenFrame = frame;
    handedAt.delete(frame.timestamp);
    received.delete(frame.timestamp);
    return;
  }
  const now = performance.now();
  if (canvas && (upscaler || context)) {
    if (frame.displayWidth !== pictureWidth || frame.displayHeight !== pictureHeight) {
      pictureWidth = frame.displayWidth;
      pictureHeight = frame.displayHeight;
      scope.postMessage({ type: 'picture', width: pictureWidth, height: pictureHeight });
    }
    if (upscaler) {
      upscaled(canvas, frame, upscaler);
    } else if (context) {
      if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
        canvas.width = frame.displayWidth;
        canvas.height = frame.displayHeight;
      }
      context.drawImage(frame, 0, 0);
      drawnMode = 'off';
    }
  }
  frames += 1;
  const started = handedAt.get(frame.timestamp);
  handedAt.delete(frame.timestamp);
  if (started !== undefined) decodeTimes.push({ at: now, ms: now - started });
  // The frame's timestamp is its capture time on the board's clock.
  const arrived = received.get(frame.timestamp);
  received.delete(frame.timestamp);
  const wait = arrived === undefined ? null : now - arrived;
  drawn.push({ at: now, wait });
  if (wait !== null) {
    probe.waits[Math.min(WAIT_BUCKETS, Math.max(0, Math.round(wait * 10)))] += 1;
  }
  probe.frames = frames;
  probe.lastCaptureMicros = frame.timestamp;
  unaged.push({ at: now, capture: frame.timestamp / 1000 });
  if (unaged.length > MAX_UNAGED) settleAges();
  frame.close();
}

function age(value: number) {
  aged.push({ at: performance.now(), age: value });
  probe.ages[Math.min(AGE_BUCKETS, Math.max(0, Math.round(value)))] += 1;
}

/** Tells the age of the frames that the pings enclose by now. */
function settleAges() {
  const span = clock.span();
  let settled = 0;
  if (span) {
    for (; settled < unaged.length; settled += 1) {
      const frame = unaged[settled];
      if (frame.at > span.to) break;
      const board = clock.between(frame.at);
      // Drawn before the first ping was answered: its age is not known.
      if (board !== null) age(board - frame.capture);
    }
  }
  unaged = unaged.slice(settled);
  // The pings do not come: the newest of them say what they can.
  while (unaged.length > MAX_UNAGED) {
    const frame = unaged.shift();
    const board = frame ? clock.at(frame.at) : null;
    if (frame && board !== null) age(board - frame.capture);
  }
}

function failed() {
  closeDecoder();
  resets += 1;
  handle(receiver.decoderFailed(performance.now()));
}

function configure(codec: string) {
  closeDecoder();
  const created = new VideoDecoder({
    output: (frame) => timed(() => draw(frame)),
    error: () => {
      // An error of a decoder that was replaced meanwhile is not this one's.
      if (decoder === created) failed();
    }
  });
  created.configure(decoderConfig(codec));
  decoder = created;
  configured = codec;
}

/** Whether the decoder has fallen behind: a frame handed to it long ago not yet come back. */
function fallenBehind(now: number): boolean {
  for (const at of handedAt.values()) if (now - at > DECODE_LAG_MS) return true;
  return false;
}

/** Hands a frame to the decoder. */
function decode(frame: ReceivedFrame) {
  try {
    if (frame.key) {
      const codec = codecOf(frame.data);
      if (!codec) return failed();
      if (!decoder || decoder.state !== 'configured' || codec !== configured) configure(codec);
    }
    if (!decoder || decoder.state !== 'configured') return;
    if (
      !frame.key &&
      (decoder.decodeQueueSize >= MAX_DECODE_QUEUE || fallenBehind(performance.now()))
    ) {
      return failed();
    }
    received.set(frame.captureMicros, frame.receivedAt);
    handedAt.set(frame.captureMicros, performance.now());
    handed += 1;
    if (visible) watchdog?.receive(performance.now());
    decoder.decode(
      new EncodedVideoChunk({
        type: frame.key ? 'key' : 'delta',
        timestamp: frame.captureMicros,
        data: frame.data
      })
    );
  } catch {
    failed();
  }
}

/** The time this worker's own handlers take: how busy it is. */
const work = new Spans();

function timed<T>(run: () => T): T {
  const began = performance.now();
  try {
    return run();
  } finally {
    work.add(began, performance.now());
  }
}

function handle(output: ReceiverOutput) {
  for (const frame of output.frames) decode(frame);
  if (output.request) scope.postMessage({ type: 'request', ...output.request });
}

function percentile(sorted: number[], share: number): number {
  return sorted[Math.min(sorted.length - 1, Math.floor(share * sorted.length))];
}

setInterval(() => timed(() => handle(receiver.poll(performance.now()))), POLL_MS);

setInterval(() => {
  const now = performance.now();
  if (watchdog && !watchdogTold && watchdog.losing(now)) {
    watchdogTold = true;
    scope.postMessage({ type: 'upscaler', state: 'slow' });
  }
  drawn = drawn.filter((frame) => now - frame.at <= WINDOW_MS);
  drawTimes = drawTimes.filter((frame) => now - frame.at <= WINDOW_MS);
  gpuTimes = gpuTimes.filter((frame) => now - frame.at <= WINDOW_MS);
  const drawSorted = drawTimes.map((frame) => frame.ms).sort((left, right) => left - right);
  const gpuSorted = gpuTimes.map((frame) => frame.ms).sort((left, right) => left - right);
  aged = aged.filter((frame) => now - frame.at <= WINDOW_MS);
  decodeTimes = decodeTimes.filter((frame) => now - frame.at <= WINDOW_MS);
  const decodeSorted = decodeTimes.map((frame) => frame.ms).sort((left, right) => left - right);
  const known = aged.map((frame) => frame.age).sort((left, right) => left - right);
  const waits = drawn
    .map((frame) => frame.wait)
    .filter((wait): wait is number => wait !== null)
    .sort((left, right) => left - right);
  scope.postMessage({
    type: 'stats',
    frames,
    framesPerSecond: drawn.filter((frame) => now - frame.at <= STATS_MS).length,
    frameRate: Math.round((drawn.length * 10000) / WINDOW_MS) / 10,
    ageMedianMs: known.length ? percentile(known, 0.5) : null,
    ageP95Ms: known.length ? percentile(known, 0.95) : null,
    waitMedianMs: waits.length ? percentile(waits, 0.5) : null,
    dropped: receiver.counters.dropped,
    requests: receiver.counters.requests,
    resets,
    queued: decoder?.decodeQueueSize ?? 0,
    bytes,
    received: handed,
    decodeMedianMs: decodeSorted.length ? percentile(decodeSorted, 0.5) : null,
    upscale: {
      mode: drawnMode,
      drawMs: drawSorted.length ? percentile(drawSorted, 0.5) : null,
      gpuMs: gpuSorted.length ? percentile(gpuSorted, 0.5) : null,
      frames: watchdog ? watchdog.rates(now) : null
    },
    canvasWidth: canvas?.width ?? 0,
    canvasHeight: canvas?.height ?? 0,
    busy: work.share(now)
  });
}, STATS_MS);

scope.onmessage = (event) => timed(() => receive(event));

function receive(event: MessageEvent<ToVideoWorker>) {
  const message = event.data;
  const now = performance.now();
  switch (message.type) {
    case 'canvas':
      canvas = message.canvas;
      if (message.upscale !== 'off') {
        sharpness = message.upscale;
        upscaler = Upscaler.on(canvas, {
          onLost: () => scope.postMessage({ type: 'upscaler', state: 'lost' })
        });
        if (upscaler) watchdog = new UpscaleWatchdog(now);
        else scope.postMessage({ type: 'upscaler', state: 'unavailable' });
      } else {
        context = canvas.getContext('2d');
      }
      break;
    case 'view':
      view = { width: message.width, height: message.height, pixelRatio: message.pixelRatio };
      break;
    case 'sharpness':
      sharpness = message.sharpness;
      break;
    case 'visible':
      visible = message.visible;
      if (visible) {
        // Counted afresh: nothing was drawn while the page was not seen.
        watchdog?.reset(now);
        const frame = hiddenFrame;
        hiddenFrame = null;
        if (frame) draw(frame);
      }
      break;
    case 'begin':
      receiver.reset();
      clock.reset();
      unaged = [];
      closeDecoder();
      watchdog?.reset(now);
      break;
    case 'datagram':
      bytes += message.data.byteLength;
      handle(receiver.datagram(new Uint8Array(message.data), now));
      break;
    case 'keyframe':
      bytes += message.data.byteLength;
      handle(receiver.keyFrame(new Uint8Array(message.data), now));
      break;
    case 'groupframe':
      bytes += message.data.byteLength;
      handle(receiver.groupFrame(new Uint8Array(message.data), now));
      break;
    case 'pong': {
      // When the answer arrived, on this worker's clock; the middle of its
      // round trip was half of it before.
      const arrived = message.at - performance.timeOrigin;
      clock.sample(
        arrived - message.roundTripMs / 2,
        message.boardMicros / 1000,
        message.roundTripMs
      );
      settleAges();
      break;
    }
  }
}
