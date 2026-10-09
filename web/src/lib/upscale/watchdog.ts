/**
 * The upscaler's watchdog (docs/design/screen-upscaling-v1.md, "The
 * safeguards"): while a view draws through the upscaler, the frames it
 * received and the frames it drew over the last five seconds are counted;
 * when fewer than 90 % of them were drawn, upscaling stops for the page load
 * and the view goes on plainly. A view counts as received only what it could
 * have drawn (QUIC: the frames handed to the decoder, not those the network
 * lost), so that a bad path is not taken for a slow upscaler.
 */

/** Why upscaling stopped on this page load. */
export type UpscaleStop =
  // No WebGL2 where the view draws.
  | 'unavailable'
  // The context was lost.
  | 'lost'
  // The watchdog: frames were lost to it.
  | 'slow';

/** The span the counts cover. */
export const WATCH_MS = 5000;
/** The share of the frames received that must be drawn. */
export const DRAWN_SHARE = 0.9;
/** Fewer frames than this tell nothing: a still screen sends one every two seconds. */
export const MIN_RECEIVED = 15;

export class UpscaleWatchdog {
  private received: number[] = [];
  private drawn: number[] = [];
  private since: number;

  constructor(now: number) {
    this.since = now;
  }

  /** Counting starts over: a new session, whose frames came before were lost with the old. */
  reset(now: number): void {
    this.received = [];
    this.drawn = [];
    this.since = now;
  }

  receive(now: number): void {
    this.received.push(now);
  }

  draw(now: number): void {
    this.drawn.push(now);
  }

  /**
   * Whether frames were lost to the upscaler: over the last five seconds,
   * all of them counted, fewer than 90 % of the frames received were drawn.
   */
  losing(now: number): boolean {
    const from = now - WATCH_MS;
    this.received = this.received.filter((at) => at > from);
    this.drawn = this.drawn.filter((at) => at > from);
    if (now - this.since < WATCH_MS || this.received.length < MIN_RECEIVED) return false;
    return this.drawn.length < DRAWN_SHARE * this.received.length;
  }
}
