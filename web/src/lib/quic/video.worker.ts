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
  /** A ping answered: its round trip, and the board's clock in the answer. */
  | { type: 'pong'; roundTripMs: number; boardMicros: number };

export type VideoStats = {
  /** Frames drawn since the picture began, and in the last second. */
  frames: number;
  framesPerSecond: number;
  /** Capture to drawing by the board's clock, over the last second. */
  ageMedianMs: number | null;
  ageP95Ms: number | null;
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
let lastSecond: { at: number; age: number | null }[] = [];

function closeDecoder() {
  const old = decoder;
  decoder = null;
  configured = null;
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
  // The frame's timestamp is its capture time on the board's clock.
  const board = clock.at(now);
  frames += 1;
  lastSecond.push({ at: now, age: board === null ? null : board - frame.timestamp / 1000 });
  frame.close();
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
  lastSecond = lastSecond.filter((entry) => now - entry.at <= STATS_MS);
  const ages = lastSecond
    .map((entry) => entry.age)
    .filter((age): age is number => age !== null)
    .sort((left, right) => left - right);
  scope.postMessage({
    type: 'stats',
    frames,
    framesPerSecond: lastSecond.length,
    ageMedianMs: ages.length ? percentile(ages, 0.5) : null,
    ageP95Ms: ages.length ? percentile(ages, 0.95) : null,
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
      closeDecoder();
      break;
    case 'datagram':
      handle(receiver.datagram(new Uint8Array(message.data), now));
      break;
    case 'keyframe':
      handle(receiver.keyFrame(new Uint8Array(message.data), now));
      break;
    case 'pong':
      // The answer has just arrived: the middle of its round trip was half
      // of it ago.
      clock.sample(now - message.roundTripMs / 2, message.boardMicros / 1000, message.roundTripMs);
      break;
  }
};
