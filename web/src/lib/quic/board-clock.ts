/**
 * The board's monotonic clock as this page can tell it from its pings: each
 * answered ping gives the board's time at about the middle of the round
 * trip. The samples with the shortest round trips say best what the time
 * was; a line through them gives the offset and the rate, so that a host
 * whose clock runs faster or slower than the board's does not skew the age
 * of a frame.
 */

/** Samples kept; with a ping every two seconds, about two minutes. */
export const MAX_SAMPLES = 64;
/** A sample this much slower than the fastest is not used. */
const ROUND_TRIP_SLACK_MS = 3;
/** Below this span the samples say nothing about the rate. */
const MIN_RATE_SPAN_MS = 8000;
/** A rate further from one than this is not believed. */
const MAX_RATE_ERROR = 0.2;

type Sample = { page: number; board: number; roundTrip: number };

export class BoardClock {
  private samples: Sample[] = [];
  private line: { offset: number; rate: number; at: number } | null = null;

  /**
   * A ping answered: `pageMiddle` is this page's clock at the middle of the
   * round trip, `boardMs` the board's clock in the answer, both in
   * milliseconds.
   */
  sample(pageMiddle: number, boardMs: number, roundTripMs: number): void {
    if (!(roundTripMs >= 0) || !Number.isFinite(pageMiddle) || !Number.isFinite(boardMs)) return;
    this.samples.push({ page: pageMiddle, board: boardMs, roundTrip: roundTripMs });
    if (this.samples.length > MAX_SAMPLES) this.samples.shift();
    this.fit();
  }

  /** The board's clock at `page`, or `null` before the first sample. */
  at(page: number): number | null {
    const line = this.line;
    return line ? line.offset + line.rate * (page - line.at) : null;
  }

  /** How fast the board's clock runs against the page's. */
  rate(): number | null {
    return this.line?.rate ?? null;
  }

  /** Forgets the samples: after a new session, the board may be another. */
  reset(): void {
    this.samples = [];
    this.line = null;
  }

  private fit(): void {
    const fastest = Math.min(...this.samples.map((sample) => sample.roundTrip));
    const good = this.samples.filter((sample) => sample.roundTrip <= fastest + ROUND_TRIP_SLACK_MS);
    const newest = good[good.length - 1];
    const span = newest.page - good[0].page;
    if (good.length >= 3 && span >= MIN_RATE_SPAN_MS) {
      // Least squares, about the means so the numbers stay small.
      const meanPage = good.reduce((sum, sample) => sum + sample.page, 0) / good.length;
      const meanBoard = good.reduce((sum, sample) => sum + sample.board, 0) / good.length;
      let over = 0;
      let under = 0;
      for (const sample of good) {
        over += (sample.page - meanPage) * (sample.board - meanBoard);
        under += (sample.page - meanPage) ** 2;
      }
      const fitted = under > 0 ? over / under : 1;
      if (Math.abs(fitted - 1) <= MAX_RATE_ERROR) {
        this.line = { offset: meanBoard, rate: fitted, at: meanPage };
        return;
      }
    }
    // Too few samples for a rate: the best of them gives the offset.
    const best = good.reduce((left, right) => (right.roundTrip <= left.roundTrip ? right : left));
    this.line = { offset: best.board, rate: 1, at: best.page };
  }
}
