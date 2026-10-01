/**
 * The board's monotonic clock as this page can tell it from its pings: each
 * answered ping gives the board's time at about the middle of the round
 * trip. The pings with the shortest round trips say best what the time was;
 * they are kept as the points of a curve. The board's time at a moment
 * between two of them is read between them, so the page needs to know
 * neither how far the two clocks are apart nor how fast one runs against the
 * other, and a host whose clock changes its rate from one second to the next
 * (a virtual machine's does) does not skew the age of a frame.
 */

/** Points kept. */
export const MAX_POINTS = 32;
/** The fastest round trip is the fastest of this many of the newest pings. */
const RECENT_PINGS = 20;
/** A ping this much slower than the fastest is not used. */
const ROUND_TRIP_SLACK_MS = 2;
/** For a moment after the newest point: the rate between it and one at least this much older. */
const RATE_SPAN_MS = 1000;
/** A rate further from one than this is not believed. */
const MAX_RATE_ERROR = 0.2;

type Point = { page: number; board: number };

export class BoardClock {
  private roundTrips: number[] = [];
  private points: Point[] = [];

  /**
   * A ping answered: `pageMiddle` is this page's clock at the middle of the
   * round trip, `boardMs` the board's clock in the answer, both in
   * milliseconds.
   */
  sample(pageMiddle: number, boardMs: number, roundTripMs: number): void {
    if (!(roundTripMs >= 0) || !Number.isFinite(pageMiddle) || !Number.isFinite(boardMs)) return;
    this.roundTrips.push(roundTripMs);
    if (this.roundTrips.length > RECENT_PINGS) this.roundTrips.shift();
    if (roundTripMs > Math.min(...this.roundTrips) + ROUND_TRIP_SLACK_MS) return;
    const newest = this.points[this.points.length - 1];
    // Both clocks only go forward: an answer that says otherwise is not one.
    if (newest && (pageMiddle <= newest.page || boardMs < newest.board)) return;
    this.points.push({ page: pageMiddle, board: boardMs });
    if (this.points.length > MAX_POINTS) this.points.shift();
  }

  /** The moments the points enclose, or `null` with fewer than two points. */
  span(): { from: number; to: number } | null {
    if (this.points.length < 2) return null;
    return { from: this.points[0].page, to: this.points[this.points.length - 1].page };
  }

  /**
   * The board's clock at `page`, read between the two points around it;
   * `null` if the points do not enclose it.
   */
  between(page: number): number | null {
    const points = this.points;
    if (points.length < 2 || page < points[0].page) return null;
    for (let index = points.length - 1; index > 0; index -= 1) {
      const after = points[index];
      const before = points[index - 1];
      if (page > after.page) return null;
      if (page < before.page) continue;
      const share = (page - before.page) / (after.page - before.page);
      return before.board + share * (after.board - before.board);
    }
    return null;
  }

  /**
   * The board's clock at `page` by the line through the newest points: for
   * a moment they do not enclose. `null` before the first point.
   */
  at(page: number): number | null {
    const newest = this.points[this.points.length - 1];
    if (!newest) return null;
    return newest.board + (this.rate() ?? 1) * (page - newest.page);
  }

  /** How fast the board's clock runs against the page's, by the newest points. */
  rate(): number | null {
    const points = this.points;
    const newest = points[points.length - 1];
    if (!newest) return null;
    // The newest point that is old enough to tell a rate.
    for (let index = points.length - 2; index >= 0; index -= 1) {
      const older = points[index];
      if (newest.page - older.page < RATE_SPAN_MS) continue;
      const rate = (newest.board - older.board) / (newest.page - older.page);
      return Math.abs(rate - 1) <= MAX_RATE_ERROR ? rate : 1;
    }
    return 1;
  }

  /** Forgets the points: after a new session, the board may be another. */
  reset(): void {
    this.roundTrips = [];
    this.points = [];
  }
}
