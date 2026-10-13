import { mapToDesktop, type ScreenArea } from './screen-area.ts';

// Button bit positions
const MouseButtons = {
  Left: 1 << 0,
  Right: 1 << 1,
  Middle: 1 << 2,
  Back: 1 << 3,
  Forward: 1 << 4
} as const;

// Map browser button index to HID bit
function getMouseButtonBit(button: number): number {
  switch (button) {
    case 0:
      return MouseButtons.Left;
    case 1:
      return MouseButtons.Middle;
    case 2:
      return MouseButtons.Right;
    case 3:
      return MouseButtons.Back;
    case 4:
      return MouseButtons.Forward;
    default:
      return 0;
  }
}

/**
 * Relative Mouse Report (4 bytes)
 * Used with /dev/hidg1 (relative mouse)
 *
 * Byte 0: Buttons
 * Byte 1: X movement (-127 to 127)
 * Byte 2: Y movement (-127 to 127)
 * Byte 3: Wheel (-127 to 127)
 */
export class MouseReportRelative {
  private buttons: number = 0;

  buttonDown(button: number): void {
    this.buttons |= getMouseButtonBit(button);
  }

  buttonUp(button: number): void {
    this.buttons &= ~getMouseButtonBit(button);
  }

  /**
   * Build relative mouse report
   * @param deltaX X movement (-127 to 127)
   * @param deltaY Y movement (-127 to 127)
   * @param wheel Scroll wheel (-127 to 127, negative = down)
   */
  buildReport(deltaX: number, deltaY: number, wheel: number = 0): Uint8Array {
    const report = new Uint8Array(4);
    report[0] = this.buttons;
    report[1] = this.clamp(Math.round(deltaX), -127, 127) & 0xff;
    report[2] = this.clamp(Math.round(deltaY), -127, 127) & 0xff;
    report[3] = this.clamp(Math.round(wheel), -127, 127) & 0xff;
    return report;
  }

  /**
   * Build button-only report (no movement)
   */
  buildButtonReport(): Uint8Array {
    return this.buildReport(0, 0, 0);
  }

  reset(): Uint8Array {
    this.buttons = 0;
    return this.buildReport(0, 0, 0);
  }

  private clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value));
  }
}

/**
 * Absolute Mouse Report (6 bytes)
 * Used with /dev/hidg2 (absolute mouse/tablet)
 *
 * Byte 0: Buttons
 * Byte 1-2: X position (0 to 32767, Little Endian)
 * Byte 3-4: Y position (0 to 32767, Little Endian)
 * Byte 5: Wheel
 */
/** The largest absolute position the report carries on each axis. */
export const ABSOLUTE_MAX = 0x7fff;

export class MouseReportAbsolute {
  private buttons: number = 0;

  /**
   * `area`: where the captured screen lies in the target's desktop, asked
   * for every report (docs/design/screen-area-v1.md); positions are the
   * picture's, and the report carries them for the desktop.
   */
  private readonly area: () => ScreenArea | null;

  constructor(area: () => ScreenArea | null = () => null) {
    this.area = area;
  }

  buttonDown(button: number): void {
    this.buttons |= getMouseButtonBit(button);
  }

  buttonUp(button: number): void {
    this.buttons &= ~getMouseButtonBit(button);
  }

  /**
   * Build absolute mouse report
   * @param pictureX X position on the picture (0 to ABSOLUTE_MAX)
   * @param pictureY Y position on the picture (0 to ABSOLUTE_MAX)
   * @param wheel Scroll wheel (-127 to 127)
   */
  buildReport(pictureX: number, pictureY: number, wheel: number = 0): Uint8Array {
    const { x, y } = mapToDesktop(this.area(), pictureX, pictureY, ABSOLUTE_MAX);
    const report = new Uint8Array(6);

    report[0] = this.buttons;
    report[1] = x & 0xff;
    report[2] = (x >> 8) & 0xff;
    report[3] = y & 0xff;
    report[4] = (y >> 8) & 0xff;
    report[5] = this.clamp(Math.round(wheel), -127, 127) & 0xff;

    return report;
  }

  /**
   * Build button-only report (keeps last position)
   */
  buildButtonReport(lastX: number, lastY: number): Uint8Array {
    return this.buildReport(lastX, lastY, 0);
  }

  reset(): Uint8Array {
    this.buttons = 0;
    return this.buildReport(0, 0, 0);
  }

  private clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(max, value));
  }
}

// Singleton instances
export const mouseRelative = new MouseReportRelative();
export const mouseAbsolute = new MouseReportAbsolute();
