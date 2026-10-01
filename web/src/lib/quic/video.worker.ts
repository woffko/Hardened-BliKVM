/**
 * The picture of the QUIC transport, away from the page's own thread: it
 * puts the frames together (`video-receiver.ts`), decodes them with
 * WebCodecs and draws them on the canvas it was given. The page hands it
 * what the session delivers and sends on what it asks for.
 */
import { BoardClock } from './board-clock.ts';
import { codecOf } from './h264.ts';
import { VideoReceiver, type ReceivedFrame, type ReceiverOutput } from './video-receiver.ts';

export type ToVideoWorker =
  | { type: 'canvas'; canvas: OffscreenCanvas }
  /** A new session or subscription: nothing received before it counts. */
  | { type: 'begin' }
  | { type: 'datagram'; data: ArrayBuffer }
  | { type: 'keyframe'; data: ArrayBuffer }
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
   * Capture to drawing by the board's clock, of the frames whose age became
   * known since the last report: a frame's age is known once a ping after
   * it has been answered.
   */
  ageMedianMs: number | null;
  ageP95Ms: number | null;
  /**
   * From a frame's arrival (its first fragment, or its key frame's stream)
   * to its drawing, over the last second: what the page itself adds.
   */
  waitMedianMs: number | null;
  /** Frames given up or impossible to decode, and key-frame requests made. */
  dropped: number;
  requests: number;
  /** Times the decoder was started again: after an error, or when it fell behind. */
  resets: number;
  /** Frames the decoder has not come to yet. */
  queued: number;
};

export type FromVideoWorker =
  | { type: 'request'; stream: number; lastFrame: number; dropped: number }
  | { type: 'picture'; width: number; height: number }
  | ({ type: 'stats' } & VideoStats);

/** How often frames that wait are looked at, and a due request is made. */
const POLL_MS = 20;
const STATS_MS = 1000;
/**
 * Frames the decoder may have before it: one that falls further behind is
 * started again from a key frame rather than show a picture that grows old.
 */
const MAX_DECODE_QUEUE = 12;
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
let decoder: VideoDecoder | null = null;
let configured: string | null = null;
let frames = 0;
let resets = 0;
/** The frames of the last second: when they were drawn, and how long after they arrived. */
let drawn: { at: number; wait: number | null }[] = [];
/** When the frames the decoder has began to arrive, by their capture time. */
const received = new Map<number, number>();
/**
 * Frames drawn after the newest answered ping: when, and their capture time
 * on the board's clock. The board's time at their drawing is read between
 * two pings, so their age waits for the next one.
 */
let unaged: { at: number; capture: number }[] = [];
/** The ages that became known since the last report. */
let aged: number[] = [];
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
  if (old && old.state !== 'closed') {
    try {
      old.close();
    } catch {
      // Closed by its own error.
    }
  }
}

function draw(frame: VideoFrame) {
  const now = performance.now();
  if (canvas && context) {
    if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
      canvas.width = frame.displayWidth;
      canvas.height = frame.displayHeight;
      scope.postMessage({
        type: 'picture',
        width: frame.displayWidth,
        height: frame.displayHeight
      });
    }
    context.drawImage(frame, 0, 0);
  }
  frames += 1;
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
  aged.push(value);
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
    output: draw,
    error: () => {
      // An error of a decoder that was replaced meanwhile is not this one's.
      if (decoder === created) failed();
    }
  });
  created.configure({ codec, optimizeForLatency: true });
  decoder = created;
  configured = codec;
}

function decode(frame: ReceivedFrame) {
  try {
    if (frame.key) {
      const codec = codecOf(frame.data);
      if (!codec) return failed();
      if (!decoder || decoder.state !== 'configured' || codec !== configured) configure(codec);
    }
    if (!decoder || decoder.state !== 'configured') return;
    if (!frame.key && decoder.decodeQueueSize >= MAX_DECODE_QUEUE) return failed();
    received.set(frame.captureMicros, frame.receivedAt);
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

function handle(output: ReceiverOutput) {
  for (const frame of output.frames) decode(frame);
  if (output.request) scope.postMessage({ type: 'request', ...output.request });
}

function percentile(sorted: number[], share: number): number {
  return sorted[Math.min(sorted.length - 1, Math.floor(share * sorted.length))];
}

setInterval(() => handle(receiver.poll(performance.now())), POLL_MS);

setInterval(() => {
  const now = performance.now();
  drawn = drawn.filter((frame) => now - frame.at <= STATS_MS);
  const known = aged.sort((left, right) => left - right);
  aged = [];
  const waits = drawn
    .map((frame) => frame.wait)
    .filter((wait): wait is number => wait !== null)
    .sort((left, right) => left - right);
  scope.postMessage({
    type: 'stats',
    frames,
    framesPerSecond: drawn.length,
    ageMedianMs: known.length ? percentile(known, 0.5) : null,
    ageP95Ms: known.length ? percentile(known, 0.95) : null,
    waitMedianMs: waits.length ? percentile(waits, 0.5) : null,
    dropped: receiver.counters.dropped,
    requests: receiver.counters.requests,
    resets,
    queued: decoder?.decodeQueueSize ?? 0
  });
}, STATS_MS);

scope.onmessage = (event) => {
  const message = event.data;
  const now = performance.now();
  switch (message.type) {
    case 'canvas':
      canvas = message.canvas;
      context = canvas.getContext('2d');
      break;
    case 'begin':
      receiver.reset();
      clock.reset();
      unaged = [];
      closeDecoder();
      break;
    case 'datagram':
      handle(receiver.datagram(new Uint8Array(message.data), now));
      break;
    case 'keyframe':
      handle(receiver.keyFrame(new Uint8Array(message.data), now));
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
};
