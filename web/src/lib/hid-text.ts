import { getModifierBit } from './keymap.ts';

/**
 * Best-effort US-layout text expansion for the restricted HID path.
 *
 * The mobile soft keyboard can only hand us a final string. The hardened
 * server has no legacy paste endpoint, so each ASCII character is expanded to a
 * press/release pair. Non-ASCII characters are skipped here; layout-specific
 * text should use the on-screen virtual keyboard, which already sends physical
 * key codes for every supported language.
 */
export type KeyStep = { modifier: number; usage: number };

const SHIFT = getModifierBit('ShiftLeft') ?? 0x02;
const LETTER_BASE = 0x04;
const DIGITS: Record<string, number> = {
  '1': 0x1e,
  '2': 0x1f,
  '3': 0x20,
  '4': 0x21,
  '5': 0x22,
  '6': 0x23,
  '7': 0x24,
  '8': 0x25,
  '9': 0x26,
  '0': 0x27
};
const PLAIN: Record<string, number> = {
  Enter: 0x28,
  ' ': 0x2c,
  '-': 0x2d,
  '=': 0x2e,
  '[': 0x2f,
  ']': 0x30,
  '\\': 0x31,
  ';': 0x33,
  "'": 0x34,
  '`': 0x35,
  ',': 0x36,
  '.': 0x37,
  '/': 0x38
};
const SHIFTED: Record<string, string> = {
  '!': '1',
  '@': '2',
  '#': '3',
  $: '4',
  '%': '5',
  '^': '6',
  '&': '7',
  '*': '8',
  '(': '9',
  ')': '0',
  _: '-',
  '+': '=',
  '{': '[',
  '}': ']',
  '|': '\\',
  ':': ';',
  '"': "'",
  '<': ',',
  '>': '.',
  '?': '/',
  '~': '`'
};

function baseStep(character: string): KeyStep | null {
  if (character === '\n' || character === '\r') {
    return { modifier: 0, usage: PLAIN.Enter };
  }
  if (character === '\t') {
    return { modifier: 0, usage: 0x2b };
  }
  const lower = character.toLowerCase();
  if (lower >= 'a' && lower <= 'z') {
    const usage = LETTER_BASE + (lower.charCodeAt(0) - 'a'.charCodeAt(0));
    return { modifier: character === lower ? 0 : SHIFT, usage };
  }
  if (character in DIGITS) {
    return { modifier: 0, usage: DIGITS[character] };
  }
  if (character in PLAIN) {
    return { modifier: 0, usage: PLAIN[character] };
  }
  if (character in SHIFTED) {
    const base = SHIFTED[character];
    if (base in DIGITS) {
      return { modifier: SHIFT, usage: DIGITS[base] };
    }
    if (base in PLAIN) {
      return { modifier: SHIFT, usage: PLAIN[base] };
    }
  }
  return null;
}

export function textToKeyReports(text: string): KeyStep[] {
  const steps: KeyStep[] = [];
  for (const character of text) {
    const step = baseStep(character);
    if (step) {
      steps.push(step);
    }
  }
  return steps;
}
