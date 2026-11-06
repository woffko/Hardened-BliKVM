/**
 * Where the captured screen lies in the target's desktop
 * (docs/design/screen-area-v1.md). A target that extends its desktop over
 * more than one monitor spreads the absolute mouse's range over all of them,
 * and the board captures one: the page then maps the picture into the part
 * of the range that is that monitor. The board keeps the layout; none means
 * the screen is the whole desktop.
 */

/** The largest size or position, in the target's pixels. */
export const MAX_DESKTOP_PIXELS = 32767;

export type ScreenArea = {
  /** The whole desktop the target spreads the absolute mouse over. */
  desktop: { width: number; height: number };
  /** The captured screen in it, from the desktop's top left corner. */
  screen: { x: number; y: number; width: number; height: number };
};

const whole = (value: unknown, least: number) =>
  typeof value === 'number' &&
  Number.isInteger(value) &&
  value >= least &&
  value <= MAX_DESKTOP_PIXELS;

/** Whether `value` is a layout: whole pixels, the screen inside the desktop. */
/** Whether two layouts are the same; null is the whole desktop. */
export function sameScreenArea(a: ScreenArea | null, b: ScreenArea | null): boolean {
  if (a === null || b === null) return a === b;
  return (
    a.desktop.width === b.desktop.width &&
    a.desktop.height === b.desktop.height &&
    a.screen.x === b.screen.x &&
    a.screen.y === b.screen.y &&
    a.screen.width === b.screen.width &&
    a.screen.height === b.screen.height
  );
}

export function isScreenArea(value: unknown): value is ScreenArea {
  if (typeof value !== 'object' || value === null) return false;
  const { desktop, screen } = value as Partial<ScreenArea>;
  if (typeof desktop !== 'object' || desktop === null) return false;
  if (typeof screen !== 'object' || screen === null) return false;
  return (
    whole(desktop.width, 1) &&
    whole(desktop.height, 1) &&
    whole(screen.x, 0) &&
    whole(screen.y, 0) &&
    whole(screen.width, 1) &&
    whole(screen.height, 1) &&
    screen.x + screen.width <= desktop.width &&
    screen.y + screen.height <= desktop.height
  );
}

/**
 * One axis of an absolute position, `value` of `max` across the picture, as
 * the target reads it across its desktop. The position stays inside the
 * screen's first and last pixel: the target reads the range as `max + 1`
 * steps over the desktop, and a picture's edge mapped onto the screen's edge
 * would be the next monitor's first pixel.
 */
function axis(value: number, max: number, from: number, size: number, total: number): number {
  const share = Math.max(0, Math.min(1, value / max));
  const pixel = Math.max(from + 0.5, Math.min(from + size - 0.5, from + share * size));
  // Down: rounded up, the screen's last pixel could become the next one's.
  return Math.max(0, Math.min(max, Math.floor((pixel / total) * (max + 1))));
}

/**
 * An absolute position of the picture, `x` and `y` of `max`, as the report
 * carries it for the target's desktop; unchanged without a layout.
 */
export function mapToDesktop(
  area: ScreenArea | null,
  x: number,
  y: number,
  max: number
): { x: number; y: number } {
  if (!area) return { x, y };
  return {
    x: axis(x, max, area.screen.x, area.screen.width, area.desktop.width),
    y: axis(y, max, area.screen.y, area.screen.height, area.desktop.height)
  };
}

/** What the page does with the target agent's layout at one look. */
export type LayoutStep = {
  /** The layout to write to the board, if one is to be; null is the whole desktop. */
  write?: { area: ScreenArea | null };
  /** The key of the layout last tried. */
  tried: string | null;
};

/**
 * The agent's layout (null while it gives none, 'whole' for the whole
 * desktop) against the one the board keeps: each layout is written once, so
 * an area the operator then keeps on the board stands. Once the agent's
 * layout goes, the one tried is forgotten, so the agent's return writes it
 * again (code audit of 2026-11-06, AUD-16).
 */
export function layoutStep(
  agent: ScreenArea | 'whole' | null,
  kept: ScreenArea | null,
  tried: string | null
): LayoutStep {
  if (agent === null) return { tried: null };
  const wanted = agent === 'whole' ? null : agent;
  if (sameScreenArea(wanted, kept)) return { tried };
  const key = JSON.stringify(wanted);
  if (tried === key) return { tried };
  return { write: { area: wanted }, tried: key };
}

/**
 * The key last tried once the write of `written` answered: a write the board
 * did not take is forgotten, so the layout is tried again.
 */
export function afterLayoutWrite(
  tried: string | null,
  written: string,
  applied: boolean
): string | null {
  return applied || tried !== written ? tried : null;
}
