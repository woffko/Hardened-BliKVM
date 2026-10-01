/**
 * This page's session of the QUIC transport
 * (`docs/design/quic-transport-v1.md`): a WebTransport session to the board's
 * gateway, opened with a token from the web server and the hash of the
 * gateway's certificate. Whatever goes wrong here, the page's standard
 * transport stays as it is.
 */
import { getDefaultStore } from 'jotai';

import { getTransport, requestQuicSession } from '@/api/transport.ts';
import { quicStatusAtom } from '@/jotai/transport.ts';

import {
  encodeMessage,
  hashBytes,
  MessageReader,
  pagePort,
  PROTOCOL,
  SESSION_PATH,
  type GatewayControl,
  type GatewayCounters
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
/** A session whose pings stay unanswered this long is lost. */
const PONG_TIMEOUT_MS = 6000;
/** After a lost session, before each attempt to come back. */
const RECONNECT_DELAYS_MS = [1000, 2000, 4000];
/** Failures that waiting may cure: worth another attempt after a loss. */
const TRANSIENT: ReadonlySet<QuicFailure> = new Set(['unavailable', 'timeout', 'handshake']);

type Open = {
  transport: WebTransport;
  writer: WritableStreamDefaultWriter<Uint8Array>;
  reader: ReadableStreamDefaultReader<Uint8Array>;
  messages: MessageReader;
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

class QuicSession {
  /** The current run; a later start or a stop makes an earlier one stale. */
  private run = 0;
  private open: Open | null = null;
  private status: QuicStatus = { state: 'off' };

  getStatus(): QuicStatus {
    return this.status;
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
  }

  private set(status: QuicStatus) {
    this.status = status;
    getDefaultStore().set(quicStatusAtom, status);
  }

  private close() {
    const open = this.open;
    this.open = null;
    if (!open) return;
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
      this.set({ state: 'connected', roundTripMs: null, counters: null });
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
          messages: new MessageReader()
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

      const finish = (ended: Ended) => {
        if (done) return;
        done = true;
        clearInterval(pinger);
        resolve(ended);
      };
      const ping = () => {
        if (performance.now() - lastPong > PONG_TIMEOUT_MS) return finish('lost');
        pingId += 1;
        open.writer
          .write(encodeMessage({ t: 'ping', id: pingId, pageTime: performance.now() }))
          .catch(() => finish('lost'));
      };
      const pinger = setInterval(ping, PING_INTERVAL_MS);
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
            case 'pong':
              lastPong = performance.now();
              roundTripMs = lastPong - message.pageTime;
              publish();
              break;
            case 'counters':
              counters = {
                roundTripMicros: message.roundTripMicros,
                packetsLost: message.packetsLost,
                framesSent: message.framesSent,
                framesDropped: message.framesDropped,
                keyFrames: message.keyFrames
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
