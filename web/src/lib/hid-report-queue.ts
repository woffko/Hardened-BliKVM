export type MouseReport =
  | { mode: 'relative_mouse'; data: number[] }
  | { mode: 'absolute_mouse'; data: number[] };

function signedByte(value: number): number {
  return value > 127 ? value - 256 : value;
}

/**
 * Coalesce movement only when doing so preserves button and wheel semantics.
 * Relative movement is additive; absolute movement keeps the latest position.
 */
export function mergeMouseReport(current: MouseReport, next: MouseReport): MouseReport | null {
  if (current.mode !== next.mode || current.data[0] !== next.data[0]) return null;
  const wheelIndex = next.mode === 'relative_mouse' ? 3 : 5;
  if (current.data[wheelIndex] !== 0 || next.data[wheelIndex] !== 0) return null;
  if (next.mode === 'absolute_mouse') return next;

  const x = signedByte(current.data[1]) + signedByte(next.data[1]);
  const y = signedByte(current.data[2]) + signedByte(next.data[2]);
  if (x < -127 || x > 127 || y < -127 || y > 127) return null;
  return {
    mode: 'relative_mouse',
    data: [next.data[0], x & 0xff, y & 0xff, 0]
  };
}

export function hasWheel(report: MouseReport): boolean {
  return report.data[report.mode === 'relative_mouse' ? 3 : 5] !== 0;
}

/**
 * Sends mouse reports with at most one in flight. Each report is one HTTP
 * request, which can take longer than the mouse produces movement; movement
 * that arrives meanwhile merges into one pending report (the newest absolute
 * position, or the sum of relative moves), so the cursor trails the hand by
 * one request instead of a growing queue. Button and wheel changes are never
 * merged away, and reports keep their order.
 */
export class MouseQueue {
  private pending: MouseReport | null = null;
  private inFlight = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private epoch = 0;
  private readonly send: (report: MouseReport) => Promise<void>;
  private readonly flushMs: number;

  // Plain fields rather than parameter properties: the node tests load this
  // file with type stripping only.
  constructor(send: (report: MouseReport) => Promise<void>, flushMs: number) {
    this.send = send;
    this.flushMs = flushMs;
  }

  push(report: MouseReport): void {
    if (hasWheel(report)) {
      this.flush();
      this.dispatch(report);
      return;
    }
    if (this.pending) {
      const merged = mergeMouseReport(this.pending, report);
      if (merged) {
        this.pending = merged;
        return;
      }
      this.flush();
    }
    this.pending = report;
    if (this.timer === null && this.inFlight === 0) {
      this.timer = setTimeout(() => this.flush(), this.flushMs);
    }
  }

  /** Sends the pending report now, behind any report already in flight. */
  flush(): void {
    this.clearTimer();
    const report = this.pending;
    this.pending = null;
    if (report) this.dispatch(report);
  }

  /** Drops the pending report and forgets reports in flight. */
  clear(): void {
    this.epoch += 1;
    this.clearTimer();
    this.pending = null;
    this.inFlight = 0;
  }

  private dispatch(report: MouseReport): void {
    const epoch = this.epoch;
    this.inFlight += 1;
    void this.send(report)
      .catch(() => undefined)
      .finally(() => {
        if (epoch !== this.epoch) return;
        this.inFlight -= 1;
        if (this.inFlight === 0 && this.pending) this.flush();
      });
  }

  private clearTimer(): void {
    if (this.timer !== null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }
}
