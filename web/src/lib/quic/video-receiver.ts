/**
 * The receiving side of the QUIC transport's picture
 * (`docs/design/quic-transport-v1.md`, "Receiver"): it puts delta frames
 * together from their datagrams, takes key frames whole from their streams,
 * hands the decoder only frames that continue the chain, and asks for a key
 * frame when the chain breaks. It holds no clock and no decoder: the caller
 * gives the time with every call and decodes what comes back.
 */
import { decodeHeader, HEADER_BYTES, MAX_DATAGRAM_BYTES, type FrameHeader } from './frame.ts';

/** Frames that may be put together at once, and their bytes. */
export const MAX_ASSEMBLING_FRAMES = 4;
export const MAX_ASSEMBLING_BYTES = 2 * 1024 * 1024;
/**
 * Frames that may wait at once, complete or not, and their bytes. A key
 * frame comes whole on a stream of its own, and on a slow or lossy path
 * after the delta frames that refer to it; those wait for it here (the
 * reference barrier) rather than be given up and a key frame asked for
 * again, which on such a path would come late in its turn.
 */
export const MAX_PENDING_FRAMES = 120;
export const MAX_PENDING_BYTES = 16 * 1024 * 1024;
/** A frame that gets no new fragment for this long is given up. */
export const PROGRESS_TIMEOUT_MS = 120;
/** A frame is given up this long after its first fragment, whatever came, while it is incomplete. */
export const LIFETIME_MS = 1000;
/** A complete frame waits this long for its key frame and the frames before it. */
export const HOLD_MS = 2000;
/** After the first key-frame request: the waits before the next ones. */
export const RETRY_DELAYS_MS = [1000, 2000, 4000];
/** After the last of them: no request for this long. */
export const QUIET_MS = 10000;

export type ReceivedFrame = {
  key: boolean;
  stream: number;
  frame: number;
  captureMicros: number;
  /**
   * When the frame began to arrive, by the time the caller gave: with its
   * first fragment, or with its key frame's stream.
   */
  receivedAt: number;
  /** The encoded frame, Annex B. */
  data: Uint8Array;
};

/** What the page asks of the gateway on the control stream. */
export type KeyframeRequest = { stream: number; lastFrame: number; dropped: number };

export type ReceiverOutput = {
  /** Frames to decode now, in this order. */
  frames: ReceivedFrame[];
  /** A key-frame request to send now, if one is due. */
  request: KeyframeRequest | null;
};

export type ReceiverCounters = {
  /** Frames handed to the decoder. */
  delivered: number;
  /** Frames given up, superseded or impossible to decode. */
  dropped: number;
  /** Datagrams and streams that were not what the header rules allow. */
  refused: number;
  /** Fragments that had arrived before, byte for byte. */
  duplicates: number;
  /** Key-frame requests made. */
  requests: number;
};

type Pending = {
  header: FrameHeader;
  /** Payload bytes of every fragment but the last, once one was seen. */
  capacity: number | null;
  parts: (Uint8Array | null)[];
  received: number;
  firstAt: number;
  progressAt: number;
  /** The whole frame, once every fragment is there, and since when. */
  complete: Uint8Array | null;
  completeAt: number;
};

type Chain = {
  stream: number;
  /** The last decoded key frame. */
  key: number;
  /** The last decoded frame. */
  last: number;
  /** A frame after `last` is gone: nothing more decodes until a key frame. */
  broken: boolean;
};

function sameBytes(left: Uint8Array, right: Uint8Array): boolean {
  if (left.length !== right.length) return false;
  for (let index = 0; index < left.length; index += 1) {
    if (left[index] !== right[index]) return false;
  }
  return true;
}

export class VideoReceiver {
  private chain: Chain | null = null;
  private readonly pending = new Map<string, Pending>();
  private pendingBytes = 0;
  /** While a key frame is needed: how many requests of this round were made, and when the next is due. */
  private recovery: { made: number; nextAt: number } | null = null;
  /** Frames dropped since the last request, for the request's count. */
  private droppedSinceRequest = 0;
  readonly counters: ReceiverCounters = {
    delivered: 0,
    dropped: 0,
    refused: 0,
    duplicates: 0,
    requests: 0
  };

  /** The stream and the last frame handed to the decoder, if any was. */
  position(): { stream: number; frame: number } | null {
    return this.chain ? { stream: this.chain.stream, frame: this.chain.last } : null;
  }

  /** Frames that wait, complete or not. */
  waiting(): number {
    return this.pending.size;
  }

  /** The bytes those frames have, or will have once complete. */
  waitingBytes(): number {
    return this.pendingBytes;
  }

  /** Frames being put together, not yet complete. */
  assembling(): number {
    let count = 0;
    for (const pending of this.pending.values()) if (!pending.complete) count += 1;
    return count;
  }

  /** Whether a key frame is being waited for. */
  recovering(): boolean {
    return this.recovery !== null;
  }

  /** A datagram as it arrived: one fragment of a delta frame. */
  datagram(bytes: Uint8Array, now: number): ReceiverOutput {
    const header = bytes.length <= MAX_DATAGRAM_BYTES ? decodeHeader(bytes) : null;
    if (!header || header.key) {
      this.counters.refused += 1;
      return this.settle(now, []);
    }
    this.fragment(header, bytes.subarray(HEADER_BYTES), now);
    return this.settle(now, this.drain());
  }

  /** A key frame as its stream delivered it: the header and the whole frame. */
  keyFrame(bytes: Uint8Array, now: number): ReceiverOutput {
    const header = decodeHeader(bytes);
    if (!header || !header.key || bytes.length !== HEADER_BYTES + header.size) {
      this.counters.refused += 1;
      return this.settle(now, []);
    }
    const chain = this.chain;
    // An older key frame that arrived late changes nothing.
    if (chain && chain.stream === header.stream && header.frame <= chain.last) {
      return this.settle(now, []);
    }
    this.chain = { stream: header.stream, key: header.frame, last: header.frame, broken: false };
    // What waited for an earlier key frame, or for another stream, is past.
    for (const [key, waiting] of this.pending) {
      if (waiting.header.stream !== header.stream || waiting.header.reference < header.frame) {
        this.forget(key, waiting);
        this.counters.dropped += 1;
      }
    }
    // A key frame newer than the gap ends the recovery.
    this.recovery = null;
    this.droppedSinceRequest = 0;
    const frame: ReceivedFrame = {
      key: true,
      stream: header.stream,
      frame: header.frame,
      captureMicros: header.captureMicros,
      receivedAt: now,
      data: bytes.subarray(HEADER_BYTES)
    };
    this.counters.delivered += 1;
    return this.settle(now, [frame, ...this.drain()]);
  }

  /** Lets time pass: frames that waited too long are given up. */
  poll(now: number): ReceiverOutput {
    return this.settle(now, []);
  }

  /** The decoder could not take a frame it was given. */
  decoderFailed(now: number): ReceiverOutput {
    if (this.chain) this.breakChain(now);
    else this.startRecovery(now);
    return this.settle(now, []);
  }

  /** Forgets everything: the next frame decoded is a key frame. */
  reset(): void {
    this.chain = null;
    this.pending.clear();
    this.pendingBytes = 0;
    this.recovery = null;
    this.droppedSinceRequest = 0;
  }

  private fragment(header: FrameHeader, payload: Uint8Array, now: number): void {
    const key = `${header.stream}:${header.frame}`;
    let pending = this.pending.get(key);
    const chain = this.chain;
    if (!pending && chain && header.stream === chain.stream) {
      // Decoded already, or one of the frames the gap took with it.
      if (header.frame <= chain.last) return;
      if (header.reference < chain.key) return;
      if (chain.broken && header.reference === chain.key) return;
    }
    if (pending) {
      const known = pending.header;
      if (
        known.fragmentCount !== header.fragmentCount ||
        known.size !== header.size ||
        known.reference !== header.reference ||
        known.captureMicros !== header.captureMicros
      ) {
        return this.contradicted(key, pending, now);
      }
    } else {
      if (header.size > MAX_ASSEMBLING_BYTES) {
        this.counters.refused += 1;
        return this.lost(header, now);
      }
      // Room is made by giving up what has waited longest: among the frames
      // being put together, and among all that wait.
      for (;;) {
        let count = 0;
        let bytes = 0;
        for (const pending of this.pending.values()) {
          if (pending.complete) continue;
          count += 1;
          bytes += pending.header.size;
        }
        if (count < MAX_ASSEMBLING_FRAMES && bytes + header.size <= MAX_ASSEMBLING_BYTES) break;
        const oldest = this.oldest(false);
        if (!oldest) break;
        this.giveUp(oldest[0], oldest[1], now);
      }
      while (
        this.pending.size >= MAX_PENDING_FRAMES ||
        this.pendingBytes + header.size > MAX_PENDING_BYTES
      ) {
        const oldest = this.oldest(true);
        if (!oldest) break;
        this.giveUp(oldest[0], oldest[1], now);
      }
      // Giving up may have broken the chain this frame belongs to.
      const after = this.chain;
      if (
        after &&
        after.broken &&
        header.stream === after.stream &&
        header.reference === after.key
      ) {
        return;
      }
      pending = {
        header,
        capacity: null,
        parts: new Array<Uint8Array | null>(header.fragmentCount).fill(null),
        received: 0,
        firstAt: now,
        progressAt: now,
        complete: null,
        completeAt: 0
      };
      this.pending.set(key, pending);
      this.pendingBytes += header.size;
    }
    if (pending.complete) {
      this.counters.duplicates += 1;
      return;
    }
    const index = header.fragmentIndex;
    const count = header.fragmentCount;
    const last = index === count - 1;
    // Every fragment but the last has the same length; the last has the rest.
    if (!last) {
      if (pending.capacity === null) {
        if (payload.length === 0 || Math.ceil(header.size / payload.length) !== count) {
          return this.contradicted(key, pending, now);
        }
        pending.capacity = payload.length;
        const tail = pending.parts[count - 1];
        if (tail && tail.length !== header.size - pending.capacity * (count - 1)) {
          return this.contradicted(key, pending, now);
        }
      } else if (payload.length !== pending.capacity) {
        return this.contradicted(key, pending, now);
      }
    } else if (count === 1) {
      if (payload.length !== header.size) return this.contradicted(key, pending, now);
    } else if (pending.capacity !== null) {
      if (payload.length !== header.size - pending.capacity * (count - 1)) {
        return this.contradicted(key, pending, now);
      }
    } else if (payload.length === 0 || payload.length > header.size) {
      return this.contradicted(key, pending, now);
    }
    const have = pending.parts[index];
    if (have) {
      if (sameBytes(have, payload)) {
        this.counters.duplicates += 1;
        return;
      }
      return this.contradicted(key, pending, now);
    }
    // The datagram's buffer is the caller's; the fragment is kept as a copy.
    pending.parts[index] = payload.slice();
    pending.received += 1;
    pending.progressAt = now;
    if (pending.received === count) {
      const whole = new Uint8Array(header.size);
      let offset = 0;
      for (const part of pending.parts) {
        whole.set(part as Uint8Array, offset);
        offset += (part as Uint8Array).length;
      }
      pending.complete = whole;
      pending.completeAt = now;
      pending.parts = [];
    }
  }

  /** Complete frames that continue the chain, in order. */
  private drain(): ReceivedFrame[] {
    const frames: ReceivedFrame[] = [];
    for (;;) {
      const chain = this.chain;
      if (!chain || chain.broken) break;
      const key = `${chain.stream}:${chain.last + 1}`;
      const next = this.pending.get(key);
      if (!next || !next.complete || next.header.reference !== chain.key) break;
      this.forget(key, next);
      chain.last = next.header.frame;
      this.counters.delivered += 1;
      frames.push({
        key: false,
        stream: next.header.stream,
        frame: next.header.frame,
        captureMicros: next.header.captureMicros,
        receivedAt: next.firstAt,
        data: next.complete
      });
    }
    return frames;
  }

  /** Gives up what waited too long, and says whether a request is due. */
  private settle(now: number, frames: ReceivedFrame[]): ReceiverOutput {
    for (const [key, pending] of [...this.pending]) {
      if (!this.pending.has(key)) continue;
      const expired = pending.complete
        ? now - pending.completeAt >= HOLD_MS
        : now - pending.progressAt >= PROGRESS_TIMEOUT_MS || now - pending.firstAt >= LIFETIME_MS;
      if (expired) this.giveUp(key, pending, now);
    }
    let request: KeyframeRequest | null = null;
    const recovery = this.recovery;
    if (recovery && now >= recovery.nextAt) {
      request = {
        stream: this.chain?.stream ?? 0,
        lastFrame: this.chain?.last ?? 0,
        dropped: this.droppedSinceRequest
      };
      this.droppedSinceRequest = 0;
      this.counters.requests += 1;
      const wait = RETRY_DELAYS_MS[recovery.made];
      if (wait === undefined) {
        recovery.made = 0;
        recovery.nextAt = now + QUIET_MS;
      } else {
        recovery.made += 1;
        recovery.nextAt = now + wait;
      }
    }
    return { frames, request };
  }

  /** The frame that began to arrive first: among all, or among the incomplete only. */
  private oldest(all: boolean): [string, Pending] | null {
    let found: [string, Pending] | null = null;
    for (const entry of this.pending) {
      if (!all && entry[1].complete) continue;
      if (!found || entry[1].firstAt < found[1].firstAt) found = entry;
    }
    return found;
  }

  private forget(key: string, pending: Pending): void {
    if (this.pending.delete(key)) this.pendingBytes -= pending.header.size;
  }

  private contradicted(key: string, pending: Pending, now: number): void {
    this.counters.refused += 1;
    this.giveUp(key, pending, now);
  }

  private giveUp(key: string, pending: Pending, now: number): void {
    this.forget(key, pending);
    this.counters.dropped += 1;
    this.droppedSinceRequest += 1;
    this.lost(pending.header, now);
  }

  /** A frame that will not be decoded: what it means for the chain. */
  private lost(header: FrameHeader, now: number): void {
    const chain = this.chain;
    if (!chain) return this.startRecovery(now);
    if (header.stream !== chain.stream || header.reference !== chain.key) {
      // It waited for a key frame that did not come.
      return this.startRecovery(now);
    }
    if (header.frame > chain.last) this.breakChain(now);
  }

  private breakChain(now: number): void {
    const chain = this.chain;
    if (!chain) return;
    if (!chain.broken) {
      chain.broken = true;
      // What depends on the lost frame cannot be decoded either.
      for (const [key, pending] of this.pending) {
        if (pending.header.stream === chain.stream && pending.header.reference === chain.key) {
          this.forget(key, pending);
          this.counters.dropped += 1;
          this.droppedSinceRequest += 1;
        }
      }
    }
    this.startRecovery(now);
  }

  private startRecovery(now: number): void {
    // One request is outstanding at a time; its schedule goes on.
    if (!this.recovery) this.recovery = { made: 0, nextAt: now };
  }
}
