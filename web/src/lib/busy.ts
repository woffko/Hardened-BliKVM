/**
 * How much of the last second the page's own picture code took, on its main
 * thread and in the picture's worker, for the quality monitor
 * (docs/design/h264-engine-choice-and-quality-monitor-v1.md). The page
 * cannot read the browser's processor use, nor the browser's own decoding,
 * compositing and drawing: this is what the page adds to it. (The idle time
 * the browser gives the main thread is no measure: in a software-drawing
 * browser its idle callbacks stopped while the thread was idle.)
 */

/** The span the share covers. */
export const BUSY_SPAN_MS = 1000;

/** Spans of time, joined where they meet, kept for the last two seconds. */
export class Spans {
  private spans: Array<[number, number]> = [];

  /** A span from `from` to `to`; spans come in the order they begin. */
  add(from: number, to: number): void {
    if (!(to > from)) return;
    const last = this.spans[this.spans.length - 1];
    if (last && from <= last[1]) last[1] = Math.max(last[1], to);
    else this.spans.push([from, to]);
    const kept = to - 2 * BUSY_SPAN_MS;
    while (this.spans.length && this.spans[0][1] < kept) this.spans.shift();
  }

  /** How much of the second before `now` the spans cover, from 0 to 1. */
  share(now: number): number {
    const from = now - BUSY_SPAN_MS;
    let covered = 0;
    for (const [start, end] of this.spans) {
      covered += Math.max(0, Math.min(end, now) - Math.max(start, from));
    }
    return Math.min(1, covered / BUSY_SPAN_MS);
  }
}

/** The time the page's own picture code takes on the main thread. */
export const pageWork = new Spans();

/** Runs `run`, counted as the page's own picture code on the main thread. */
export function counted<T>(run: () => T): T {
  const began = performance.now();
  try {
    return run();
  } finally {
    pageWork.add(began, performance.now());
  }
}
