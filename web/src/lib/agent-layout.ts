/**
 * The absolute mouse's layout from the target agent's monitors
 * (docs/design/target-agent-v1.md): the desktop the target spreads the
 * range over is the box around every active monitor, and the captured
 * screen is the monitor the agent marks as the board's, from the box's top
 * left corner. One monitor, or every monitor showing one picture, is the
 * whole desktop, whichever is the board's. Monitors extended without one
 * marked, or with one that does not make a layout, say none: the manual
 * layout, if any, stays.
 */
import { isScreenArea, type ScreenArea } from './screen-area.ts';

/** A monitor as the agent reads it: its place and size in the desktop. */
export type AgentMonitor = {
  /** The monitor as the target connects it; empty from an older agent. */
  id?: string;
  name: string;
  maker: string;
  product: number;
  x: number;
  y: number;
  width: number;
  height: number;
  refresh_millihertz: number;
  primary: boolean;
  scale: number;
  board: boolean;
};

/** A layout the agent's monitors make: part of a desktop, or the whole one. */
export type AgentLayout = ScreenArea | 'whole';

export function layoutFromMonitors(monitors: AgentMonitor[]): AgentLayout | null {
  const now = displaysNow(monitors);
  if (now === 'unknown') return null;
  if (monitors.length === 1 || now === 'duplicate') return 'whole';
  const area = areaFromMonitors(monitors);
  if (!area) return null;
  const whole =
    area.screen.x === 0 &&
    area.screen.y === 0 &&
    area.screen.width === area.desktop.width &&
    area.screen.height === area.desktop.height;
  return whole ? 'whole' : area;
}

export function areaFromMonitors(monitors: AgentMonitor[]): ScreenArea | null {
  const board = monitors.filter((monitor) => monitor.board);
  if (board.length !== 1 || monitors.length === 0) return null;
  const left = Math.min(...monitors.map((monitor) => monitor.x));
  const top = Math.min(...monitors.map((monitor) => monitor.y));
  const right = Math.max(...monitors.map((monitor) => monitor.x + monitor.width));
  const bottom = Math.max(...monitors.map((monitor) => monitor.y + monitor.height));
  const area = {
    desktop: { width: right - left, height: bottom - top },
    screen: {
      x: board[0].x - left,
      y: board[0].y - top,
      width: board[0].width,
      height: board[0].height
    }
  };
  return isScreenArea(area) ? area : null;
}

/**
 * The monitors' mode the agent's active monitors show, as Windows names its
 * choices: one desktop over them (extend), one picture on all (duplicate:
 * every monitor at the same place and size), or a single monitor, the
 * board's or another.
 */
export type DisplaysNow = 'extend' | 'duplicate' | 'board_only' | 'others_only' | 'unknown';

export function displaysNow(monitors: AgentMonitor[]): DisplaysNow {
  if (monitors.length === 0) return 'unknown';
  if (monitors.length === 1) return monitors[0].board ? 'board_only' : 'others_only';
  const [first] = monitors;
  const together = monitors.every(
    (monitor) =>
      monitor.x === first.x &&
      monitor.y === first.y &&
      monitor.width === first.width &&
      monitor.height === first.height
  );
  return together ? 'duplicate' : 'extend';
}
