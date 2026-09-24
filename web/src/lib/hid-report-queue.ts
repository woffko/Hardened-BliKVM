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
