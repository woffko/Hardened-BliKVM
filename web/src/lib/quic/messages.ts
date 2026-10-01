/**
 * The control and input messages of the QUIC transport, as the gateway reads
 * and writes them (`crates/bli-kvm-quic-protocol`, `page.rs`): a JSON object
 * preceded by its length as two bytes, most significant first.
 */

export const PROTOCOL = 1;
export const SESSION_PATH = '/kvm';
/** A control or input message, without its two length bytes. */
export const MAX_MESSAGE_BYTES = 4096;

export type Refusal = 'hello' | 'protocol' | 'token' | 'sessions' | 'unavailable';
export type Closing = 'session' | 'shutdown' | 'protocol' | 'idle';
export type VideoState = 'started' | 'stopped' | 'unavailable';

export type PageControl =
  | { t: 'hello'; protocol: number; token: string }
  | { t: 'ping'; id: number; pageTime: number }
  | { t: 'video'; subscribe: boolean }
  | { t: 'keyframe'; stream: number; lastFrame: number; dropped: number }
  | { t: 'bye' };

export type GatewayCounters = {
  roundTripMicros: number;
  packetsLost: number;
  framesSent: number;
  framesDropped: number;
  keyFrames: number;
};

export type GatewayControl =
  | { t: 'welcome'; protocol: number; maxDatagram: number }
  | { t: 'refused'; reason: Refusal }
  | { t: 'pong'; id: number; pageTime: number; boardTime: number }
  | {
      t: 'video';
      state: VideoState;
      stream?: number;
      width?: number;
      height?: number;
      reason?: string;
    }
  | ({ t: 'counters' } & GatewayCounters)
  | { t: 'closing'; reason: Closing };

/** A request of the HID lease API on the input stream, with the page's number. */
export type InputRequest = { seq: number; request: unknown };
/** The lease API's answer to the request with the same number. */
export type InputResponse = { seq: number; response: unknown };
/** What the server answers a lease request with: the API's status and body. */
export type InputAnswer = { status: number; body: unknown };

const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8', { fatal: true });

/** A message with its two length bytes in front. */
export function encodeMessage(message: unknown): Uint8Array {
  const body = encoder.encode(JSON.stringify(message));
  if (body.length === 0 || body.length > MAX_MESSAGE_BYTES) {
    throw new RangeError(`a message of ${body.length} bytes is out of range`);
  }
  const out = new Uint8Array(2 + body.length);
  out[0] = body.length >> 8;
  out[1] = body.length & 0xff;
  out.set(body, 2);
  return out;
}

/**
 * Collects the bytes of a stream and hands out its messages one by one. A
 * length out of range or a body that is not a JSON object ends the stream:
 * `take` throws.
 */
export class MessageReader {
  private buffer = new Uint8Array(0);

  push(bytes: Uint8Array): void {
    if (bytes.length === 0) return;
    const joined = new Uint8Array(this.buffer.length + bytes.length);
    joined.set(this.buffer);
    joined.set(bytes, this.buffer.length);
    this.buffer = joined;
  }

  /** The next complete message, or `null` while one is still arriving. */
  take<T = unknown>(): T | null {
    if (this.buffer.length < 2) return null;
    const length = (this.buffer[0] << 8) | this.buffer[1];
    if (length === 0 || length > MAX_MESSAGE_BYTES) {
      throw new RangeError(`a message of ${length} bytes is out of range`);
    }
    if (this.buffer.length < 2 + length) return null;
    const parsed: unknown = JSON.parse(decoder.decode(this.buffer.subarray(2, 2 + length)));
    if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
      throw new TypeError('the message is not an object');
    }
    this.buffer = this.buffer.slice(2 + length);
    return parsed as T;
  }

  /** Bytes held that are not yet a complete message. */
  get pending(): number {
    return this.buffer.length;
  }
}

/** The bytes of a certificate hash written as hexadecimal digits. */
export function hashBytes(hex: string): Uint8Array<ArrayBuffer> {
  if (!/^[0-9a-f]{64}$/.test(hex)) throw new TypeError('not a SHA-256 hash');
  const out = new Uint8Array(32);
  for (let index = 0; index < 32; index += 1) {
    out[index] = Number.parseInt(hex.slice(index * 2, index * 2 + 2), 16);
  }
  return out;
}

/**
 * The port a page of this location is served from: the gateway's port must
 * be the same one, or the page's policy would not let it connect.
 */
export function pagePort(location: { port: string; protocol: string }): number {
  if (location.port) return Number(location.port);
  return location.protocol === 'https:' ? 443 : 80;
}
