import { getModifierBit } from './keymap.ts';

/**
 * Modifiers of the extra keys row on a phone. They are held on the target
 * like the keys of a physical keyboard, so they also go with mouse clicks:
 * a tap presses one until the next key (one shot), a second tap within
 * DOUBLE_TAP_MS keeps it pressed (locked), and a tap on a pressed one
 * releases it. The release waits DOUBLE_TAP_MS for that second tap, so a
 * double tap on a pressed modifier never releases and presses it again; a
 * lone Alt or Win press acts on its own on the target.
 */
export type ModifierMode = 'off' | 'oneShot' | 'locked';
export type ModifierModes = Readonly<Record<string, ModifierMode>>;
export type ModifierTap = 'pressed' | 'locked' | 'releasing';

export const DOUBLE_TAP_MS = 300;

export class StickyModifiers {
  private modes = new Map<string, ModifierMode>();
  private lastTap: { code: string; at: number } | null = null;

  /**
   * A tap on a modifier at `now` (milliseconds). 'releasing' changes nothing
   * yet: the caller calls release() after DOUBLE_TAP_MS, and drops that call
   * when a second tap answers 'locked' first.
   */
  tap(code: string, now: number): ModifierTap {
    const isDouble =
      this.lastTap !== null && this.lastTap.code === code && now - this.lastTap.at <= DOUBLE_TAP_MS;
    if (isDouble) {
      this.lastTap = null;
      this.modes.set(code, 'locked');
      return 'locked';
    }
    this.lastTap = { code, at: now };
    if (this.mode(code) === 'off') {
      this.modes.set(code, 'oneShot');
      return 'pressed';
    }
    return 'releasing';
  }

  /** Releases a modifier; false when it was not pressed. */
  release(code: string): boolean {
    return this.modes.delete(code);
  }

  /** A key was pressed: the one-shot modifiers are released after it. */
  consumeOneShot(): boolean {
    // A key between two taps ends the double tap.
    this.lastTap = null;
    let changed = false;
    for (const [code, mode] of this.modes) {
      if (mode === 'oneShot') {
        this.modes.delete(code);
        changed = true;
      }
    }
    return changed;
  }

  reset(): boolean {
    const changed = this.modes.size > 0;
    this.modes.clear();
    this.lastTap = null;
    return changed;
  }

  mode(code: string): ModifierMode {
    return this.modes.get(code) ?? 'off';
  }

  /** The modifier byte of a keyboard report with every pressed modifier. */
  bits(): number {
    let bits = 0;
    for (const code of this.modes.keys()) bits |= getModifierBit(code);
    return bits;
  }

  snapshot(): ModifierModes {
    return Object.fromEntries(this.modes);
  }
}

/** The modifier byte for a list of modifier key codes. */
export function modifierBits(codes: readonly string[]): number {
  return codes.reduce((bits, code) => bits | getModifierBit(code), 0);
}

/** An 8-byte keyboard report with the modifiers and at most one key. */
export function keyReport(modifiers: number, usage = 0): number[] {
  return [modifiers & 0xff, 0, usage & 0xff, 0, 0, 0, 0, 0];
}
