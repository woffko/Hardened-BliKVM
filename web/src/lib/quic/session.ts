/**
 * This page's session of the QUIC transport
 * (`docs/design/quic-transport-v1.md`): a WebTransport session to the board's
 * gateway, opened with a token from the web server and the hash of the
 * gateway's certificate. Whatever goes wrong here, the page's standard
 * transport stays as it is.
 */
import { getDefaultStore } from 'jotai';

import { getTransport, requestQuicSession } from '@/api/transport.ts';
import { quicStatusAtom, quicVideoAtom } from '@/jotai/transport.ts';

import { decodeHeader } from './frame.ts';
import { GroupRecords } from './group.ts';
import {
  encodeMessage,
  hashBytes,
  MessageReader,
  pagePort,
  PROTOCOL,
  SESSION_PATH,
  type GatewayControl,
  type GatewayCounters,
  type InputAnswer,
  type InputResponse,
  type PageControl
} from './messages.ts';

export type QuicFailure =
  // What makes the page offer only the standard transport.
  | 'unsupported' // this browser has no WebTransport
  | 'origin' // the page is not served from the gateway's port
  | 'unavailable' // no gateway is on the server's link
  | 'blocked' // the firewall mode keeps the port closed
  | 'clock' // the certificate is not valid by this page's clock
  // What ends an attempt or a session; the page falls back and may retry.
  | 'timeout' // no answer in time
  | 'lost' // the session was lost and did not come back
  // What the gateway refuses; not retried automatically.
  | 'handshake' // the connection was not accepted: the certificate, or a limit
  | 'token'
  | 'protocol'
  | 'sessions'
  | 'hello';

/** The picture over the session, as the page and the gateway have it. */
export type QuicVideoStatus =
  | { state: 'off' }
  // Asked for; the first key frame has not been announced yet.
  | { state: 'starting' }
  | { state: 'playing'; stream: number; width: number; height: number }
  // The gateway has no picture for this session, with its reason.
  | { state: 'unavailable'; reason: string };

/**
 * Where the picture's bytes go: the decoder's worker. The session delivers
 * and nothing more; what is put together and drawn is the worker's.
 */
export type VideoSink = {
  /** A new session or subscription: nothing received before it counts. */
  begin(): void;
  datagram(bytes: Uint8Array): void;
  /** A key frame as its stream delivered it: the header and the frame. */
  keyFrame(bytes: Uint8Array): void;
  /** A whole delta frame of a group's stream, after its key frame: the header and the frame. */
  groupFrame(bytes: Uint8Array): void;
  /**
   * A ping answered: its round trip, the board's clock in the answer, and
   * when the answer arrived (`performance.timeOrigin + performance.now()`).
   */
  pong(roundTripMs: number, boardMicros: number, at: number): void;
};

export type QuicStatus =
  | { state: 'off' }
  | { state: 'connecting' }
  | {
      state: 'connected';
      /** The last ping's round trip, by this page's clock. */
      roundTripMs: number | null;
      counters: GatewayCounters | null;
    }
  | { state: 'failed'; reason: QuicFailure };

/** To a ready session, and from the hello to the welcome. */
const CONNECT_TIMEOUT_MS = 5000;
/**
 * A connection turned down within this time was refused; one that fails
 * later had nobody answering, whichever of the browser and this page gave up
 * first.
 */
const REFUSED_WITHIN_MS = 2000;
const PING_INTERVAL_MS = 2000;
/**
 * While the picture is watched, the board's clock is asked for this often:
 * the age of a frame is read between two answers.
 */
const CLOCK_PING_INTERVAL_MS = 250;
/** A session whose pings stay unanswered this long is lost. */
const PONG_TIMEOUT_MS = 6000;
/**
 * Datagrams the browser keeps for the page to read. A frame's fragments
 * arrive together; the default would drop most of them.
 */
const DATAGRAM_QUEUE = 4096;
/** A request on the input stream waits this long for its answer, like one over HTTPS. */
const INPUT_TIMEOUT_MS = 8000;
/** After a lost session, before each attempt to come back. */
const RECONNECT_DELAYS_MS = [1000, 2000, 4000];
/** Failures that waiting may cure: worth another attempt after a loss. */
const TRANSIENT: ReadonlySet<QuicFailure> = new Set(['unavailable', 'timeout', 'handshake']);

type Open = {
  transport: WebTransport;
  writer: WritableStreamDefaultWriter<Uint8Array>;
  reader: ReadableStreamDefaultReader<Uint8Array>;
  messages: MessageReader;
  /** The input stream, opened when the first request is made. */
  input: Promise<InputChannel> | null;
  /**
   * Unsubscriptions whose answer has not come: the gateway answers each
   * with "stopped", which may come after the page watches again (a view that
   * mounts a new canvas unwatches and watches at once) and is not news of
   * the picture.
   */
  stopsAwaited: number;
};
type Ended = 'lost' | 'session';

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** `promise`, or `fallback` if it has not settled after `ms`. */
function within<T, F>(promise: Promise<T>, ms: number, fallback: F): Promise<T | F> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => resolve(fallback), ms);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        clearTimeout(timer);
        reject(error);
      }
    );
  });
}

function shut(transport: WebTransport) {
  try {
    transport.close();
  } catch {
    // Already closed.
  }
}

/** The next message of the control stream, `null` once the stream has ended. */
async function nextMessage(open: {
  reader: ReadableStreamDefaultReader<Uint8Array>;
  messages: MessageReader;
}): Promise<GatewayControl | null> {
  for (;;) {
    const message = open.messages.take<GatewayControl>();
    if (message) return message;
    const { value, done } = await open.reader.read();
    if (done || !value) return null;
    open.messages.push(value);
  }
}

/**
 * The session's input stream: requests of the HID lease API, each answered
 * under the number it was sent with. Requests leave in the order they are
 * made; the gateway and the server keep that order.
 */
class InputChannel {
  private seq = 0;
  private failed: Error | null = null;
  private readonly waiting = new Map<
    number,
    { resolve: (answer: InputAnswer) => void; reject: (error: Error) => void }
  >();
  private readonly messages = new MessageReader();
  private readonly writer: WritableStreamDefaultWriter<Uint8Array>;

  constructor(
    writer: WritableStreamDefaultWriter<Uint8Array>,
    reader: ReadableStreamDefaultReader<Uint8Array>
  ) {
    this.writer = writer;
    void this.read(reader);
  }

  request(request: unknown, signal?: AbortSignal): Promise<InputAnswer> {
    if (this.failed) return Promise.reject(this.failed);
    if (signal?.aborted) return Promise.reject(new DOMException('aborted', 'AbortError'));
    this.seq = (this.seq + 1) >>> 0;
    const seq = this.seq;
    return new Promise((resolve, reject) => {
      const settle = () => {
        clearTimeout(timer);
        signal?.removeEventListener('abort', aborted);
        this.waiting.delete(seq);
      };
      // An abandoned request may still be served, as one over HTTPS may.
      const aborted = () => {
        settle();
        reject(new DOMException('aborted', 'AbortError'));
      };
      const timer = setTimeout(() => {
        settle();
        reject(new Error('the input request was not answered in time'));
      }, INPUT_TIMEOUT_MS);
      signal?.addEventListener('abort', aborted);
      this.waiting.set(seq, {
        resolve: (answer) => {
          settle();
          resolve(answer);
        },
        reject: (error) => {
          settle();
          reject(error);
        }
      });
      this.writer
        .write(encodeMessage({ seq, request }))
        .catch(() => this.fail(new Error('the input stream is closed')));
    });
  }

  /** Nothing more is answered: every request that waits fails. */
  fail(error: Error): void {
    this.failed ??= error;
    for (const waiting of [...this.waiting.values()]) waiting.reject(this.failed);
  }

  private async read(reader: ReadableStreamDefaultReader<Uint8Array>) {
    try {
      for (;;) {
        const message = this.messages.take<InputResponse>();
        if (message) {
          this.waiting.get(message.seq)?.resolve(message.response as InputAnswer);
          continue;
        }
        const { value, done } = await reader.read();
        if (done || !value) break;
        this.messages.push(value);
      }
    } catch {
      // The stream or its session ended.
    }
    this.fail(new Error('the input stream is closed'));
  }
}

async function openInput(transport: WebTransport): Promise<InputChannel> {
  const stream = await transport.createBidirectionalStream();
  return new InputChannel(stream.writable.getWriter(), stream.readable.getReader());
}

class QuicSession {
  /** The current run; a later start or a stop makes an earlier one stale. */
  private run = 0;
  private open: Open | null = null;
  private status: QuicStatus = { state: 'off' };
  /** Where the picture goes while the page watches it. */
  private sink: VideoSink | null = null;
  /** Sessions opened since the page was loaded: one more for every one lost and opened again. */
  private opened = 0;

  getStatus(): QuicStatus {
    return this.status;
  }

  /** Whether a session is open: the HID lease can go over it. */
  isOpen(): boolean {
    return this.open !== null;
  }

  /** For the quality monitor: the sessions opened since the page was loaded. */
  sessionsOpened(): number {
    return this.opened;
  }

  /**
   * One request of the HID lease API over the session's input stream. It
   * fails when the session is not open or ends before the answer.
   */
  async input(request: unknown, signal?: AbortSignal): Promise<InputAnswer> {
    const open = this.open;
    if (!open) throw new Error('the QUIC session is not open');
    if (!open.input) {
      const opening = openInput(open.transport);
      open.input = opening;
      opening.catch(() => {
        if (open.input === opening) open.input = null;
      });
    }
    return (await open.input).request(request, signal);
  }

  /**
   * The page watches the picture: it is asked for now if a session is open,
   * and again on every session that opens while the page watches.
   */
  watch(sink: VideoSink): void {
    this.sink = sink;
    if (this.open) this.subscribe(this.open);
  }

  unwatch(): void {
    if (!this.sink) return;
    this.sink = null;
    this.setVideo({ state: 'off' });
    if (this.open) this.open.stopsAwaited += 1;
    this.say({ t: 'video', subscribe: false });
  }

  /** The decoder lost the chain: the gateway is asked for a key frame. */
  requestKeyFrame(stream: number, lastFrame: number, dropped: number): void {
    this.say({ t: 'keyframe', stream, lastFrame, dropped });
  }

  private say(message: PageControl): void {
    this.open?.writer.write(encodeMessage(message)).catch(() => undefined);
  }

  private setVideo(video: QuicVideoStatus): void {
    getDefaultStore().set(quicVideoAtom, video);
  }

  private subscribe(open: Open): void {
    if (this.open !== open || !this.sink) return;
    this.sink.begin();
    this.setVideo({ state: 'starting' });
    this.say({ t: 'video', subscribe: true });
  }

  /**
   * Reads the session's datagrams and key-frame streams for as long as it
   * lasts, and hands them to whoever watches.
   */
  private deliver(open: Open): void {
    const transport = open.transport;
    try {
      transport.datagrams.incomingHighWaterMark = DATAGRAM_QUEUE;
    } catch {
      // The browser keeps its own limit.
    }
    const datagrams = async () => {
      const reader = transport.datagrams.readable.getReader();
      for (;;) {
        const { value, done } = await reader.read();
        if (done) return;
        if (this.open === open) this.sink?.datagram(value);
      }
    };
    // A group's stream: its key frame, then its delta frames, each handed
    // on as soon as it is whole and said to the gateway, which judges by it
    // what is still on its way; as it arrives, not by a timer, which a
    // hidden page's browser slows down. A stream that is reset was overtaken
    // by the next group, or fell behind.
    const group = async (stream: ReadableStream<Uint8Array>) => {
      const reader = stream.getReader();
      const records = new GroupRecords();
      let first = true;
      for (;;) {
        const { value, done } = await reader.read();
        if (done) return;
        let whole: Uint8Array[];
        try {
          whole = records.push(value);
        } catch {
          return reader.cancel().catch(() => undefined);
        }
        for (const record of whole) {
          if (this.open !== open) return reader.cancel().catch(() => undefined);
          const key = first;
          first = false;
          if (!this.sink) continue;
          const capture = decodeHeader(record)?.captureMicros ?? 0;
          open.writer.write(encodeMessage({ t: 'received', capture })).catch(() => undefined);
          if (key) this.sink.keyFrame(record);
          else this.sink.groupFrame(record);
        }
      }
    };
    const streams = async () => {
      const reader = transport.incomingUnidirectionalStreams.getReader();
      for (;;) {
        const { value, done } = await reader.read();
        if (done) return;
        group(value as ReadableStream<Uint8Array>).catch(() => undefined);
      }
    };
    datagrams().catch(() => undefined);
    streams().catch(() => undefined);
  }

  /** Opens the session, and keeps it open until `stop`. */
  start(): void {
    this.close();
    this.run += 1;
    void this.keep(this.run);
  }

  stop(): void {
    this.run += 1;
    this.close();
    this.set({ state: 'off' });
    if (this.sink) this.setVideo({ state: 'off' });
  }

  private set(status: QuicStatus) {
    this.status = status;
    getDefaultStore().set(quicStatusAtom, status);
  }

  private close() {
    const open = this.open;
    this.open = null;
    if (!open) return;
    open.input?.then(
      (input) => input.fail(new Error('the QUIC session ended')),
      () => undefined
    );
    open.writer.write(encodeMessage({ t: 'bye' })).catch(() => undefined);
    shut(open.transport);
  }

  private async keep(run: number) {
    this.set({ state: 'connecting' });
    let attempt = await this.connect();
    if (run !== this.run) return this.discard(attempt);
    if ('failure' in attempt) return this.set({ state: 'failed', reason: attempt.failure });

    for (;;) {
      this.open = attempt;
      this.opened += 1;
      this.set({ state: 'connected', roundTripMs: null, counters: null });
      this.deliver(attempt);
      this.subscribe(attempt);
      const ended = await this.serve(attempt);
      if (run !== this.run) return;
      this.close();
      // The web session ended. The server says so to the next request, and
      // the page then asks for a login; one is made now rather than whenever.
      if (ended === 'session') {
        this.set({ state: 'off' });
        getTransport().catch(() => undefined);
        return;
      }

      this.set({ state: 'connecting' });
      let again: Open | null = null;
      for (const delay of RECONNECT_DELAYS_MS) {
        await sleep(delay);
        if (run !== this.run) return;
        const next = await this.connect();
        if (run !== this.run) return this.discard(next);
        if (!('failure' in next)) {
          again = next;
          break;
        }
        if (!TRANSIENT.has(next.failure)) {
          return this.set({ state: 'failed', reason: next.failure });
        }
      }
      if (!again) return this.set({ state: 'failed', reason: 'lost' });
      attempt = again;
    }
  }

  private discard(attempt: Open | { failure: QuicFailure }) {
    if (!('failure' in attempt)) shut(attempt.transport);
  }

  /** One session from its token to its welcome, or why there is none. */
  private async connect(): Promise<Open | { failure: QuicFailure }> {
    if (typeof WebTransport === 'undefined') return { failure: 'unsupported' };
    // A connection that is turned down at once is tried once more with a
    // fresh token and hash: the gateway may just have replaced its
    // certificate.
    for (let attempt = 0; attempt < 2; attempt += 1) {
      let grant;
      try {
        const response = await requestQuicSession();
        if (response.code !== 0) return { failure: 'unavailable' };
        grant = response.data;
      } catch {
        return { failure: 'unavailable' };
      }
      if (!grant.granted) return { failure: grant.reason };
      // The page's policy lets it connect to its own origin only.
      if (grant.port !== pagePort(window.location)) return { failure: 'origin' };

      let transport: WebTransport;
      try {
        transport = new WebTransport(
          `https://${window.location.hostname}:${grant.port}${SESSION_PATH}`,
          {
            serverCertificateHashes: [
              { algorithm: 'sha-256', value: hashBytes(grant.certificateSha256) }
            ]
          }
        );
      } catch {
        return { failure: 'unsupported' };
      }
      // A session that ends before it was used must not raise an unhandled
      // rejection.
      transport.closed.catch(() => undefined);
      const startedAt = performance.now();
      const ready = await within(
        transport.ready.then(
          () => 'ready' as const,
          () => 'refused' as const
        ),
        CONNECT_TIMEOUT_MS,
        'timeout' as const
      );
      if (ready === 'timeout') {
        shut(transport);
        return { failure: 'timeout' };
      }
      if (ready === 'refused') {
        if (performance.now() - startedAt > REFUSED_WITHIN_MS) return { failure: 'timeout' };
        if (attempt === 0) continue;
        return { failure: 'handshake' };
      }

      try {
        const stream = await transport.createBidirectionalStream();
        const open: Open = {
          transport,
          writer: stream.writable.getWriter(),
          reader: stream.readable.getReader(),
          messages: new MessageReader(),
          input: null,
          stopsAwaited: 0
        };
        await open.writer.write(
          encodeMessage({ t: 'hello', protocol: PROTOCOL, token: grant.token })
        );
        const first = await within(nextMessage(open), CONNECT_TIMEOUT_MS, 'timeout' as const);
        if (first !== 'timeout' && first?.t === 'welcome' && first.protocol === PROTOCOL) {
          return open;
        }
        shut(transport);
        if (first === 'timeout') return { failure: 'timeout' };
        if (first?.t === 'refused') return { failure: first.reason };
        return { failure: 'protocol' };
      } catch {
        shut(transport);
        return { failure: 'handshake' };
      }
    }
    return { failure: 'handshake' };
  }

  /** Pings, and reads what the gateway says, until the session ends. */
  private serve(open: Open): Promise<Ended> {
    return new Promise((resolve) => {
      let done = false;
      let lastPong = performance.now();
      let pingId = 0;
      let roundTripMs: number | null = null;
      let counters: GatewayCounters | null = null;

      // Pings that ask for the board's clock only.
      const clockPings = new Set<number>();

      const finish = (ended: Ended) => {
        if (done) return;
        done = true;
        clearInterval(pinger);
        clearInterval(clockPinger);
        resolve(ended);
      };
      const send = () => {
        pingId += 1;
        open.writer
          .write(encodeMessage({ t: 'ping', id: pingId, pageTime: performance.now() }))
          .catch(() => finish('lost'));
        return pingId;
      };
      const ping = () => {
        if (performance.now() - lastPong > PONG_TIMEOUT_MS) return finish('lost');
        send();
      };
      const pinger = setInterval(ping, PING_INTERVAL_MS);
      const clockPinger = setInterval(() => {
        if (this.open === open && this.sink) clockPings.add(send());
      }, CLOCK_PING_INTERVAL_MS);
      ping();

      const publish = () => {
        if (!done && this.open === open) {
          this.set({ state: 'connected', roundTripMs, counters });
        }
      };
      const read = async () => {
        for (;;) {
          const message = await nextMessage(open);
          if (!message) return finish('lost');
          switch (message.t) {
            case 'pong': {
              lastPong = performance.now();
              const trip = lastPong - message.pageTime;
              if (this.open === open) {
                this.sink?.pong(trip, message.boardTime, performance.timeOrigin + lastPong);
              }
              if (clockPings.delete(message.id)) break;
              roundTripMs = trip;
              publish();
              break;
            }
            case 'video':
              if (message.state === 'stopped' && open.stopsAwaited > 0) {
                open.stopsAwaited -= 1;
                break;
              }
              if (this.open !== open || !this.sink) break;
              if (message.state === 'started') {
                this.setVideo({
                  state: 'playing',
                  stream: message.stream ?? 0,
                  width: message.width ?? 0,
                  height: message.height ?? 0
                });
              } else {
                // Stopped by the gateway, or never to be had: the reason.
                this.setVideo({
                  state: 'unavailable',
                  reason: message.reason ?? message.state
                });
              }
              break;
            case 'counters':
              counters = {
                roundTripMicros: message.roundTripMicros,
                packetsLost: message.packetsLost,
                framesSent: message.framesSent,
                framesDropped: message.framesDropped,
                keyFrames: message.keyFrames,
                bitrateShare: message.bitrateShare
              };
              publish();
              break;
            case 'closing':
              return finish(message.reason === 'session' ? 'session' : 'lost');
            default:
              break;
          }
        }
      };
      read().catch(() => finish('lost'));
      open.transport.closed.then(
        () => finish('lost'),
        () => finish('lost')
      );
    });
  }
}

export const quicSession = new QuicSession();
