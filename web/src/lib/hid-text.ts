import { getModifierBit } from './keymap.ts';

/**
 * Text expansion to physical key presses for the restricted HID path.
 *
 * The mobile soft keyboard can only hand us a string, and HID carries key
 * positions, not characters: the target's active layout turns them into text.
 * Latin text is sent at US positions and Cyrillic text at the positions of the
 * standard Windows Russian (JCUKEN) layout, so each script appears correctly
 * while the target uses the matching layout, exactly as with a physical
 * keyboard. Punctuation that the Russian layout moves to other keys follows
 * the script of the last letter. Other characters are skipped.
 */
export type KeyStep = { modifier: number; usage: number };

/** Script of the last letter typed; it selects the punctuation positions. */
export type TextLayoutState = { script: 'latin' | 'cyrillic' };

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

// Russian letters by the US key at the same position.
const RUSSIAN_LETTERS: Record<string, string> = {
  ё: '`',
  й: 'q',
  ц: 'w',
  у: 'e',
  к: 'r',
  е: 't',
  н: 'y',
  г: 'u',
  ш: 'i',
  щ: 'o',
  з: 'p',
  х: '[',
  ъ: ']',
  ф: 'a',
  ы: 's',
  в: 'd',
  а: 'f',
  п: 'g',
  р: 'h',
  о: 'j',
  л: 'k',
  д: 'l',
  ж: ';',
  э: "'",
  я: 'z',
  ч: 'x',
  с: 'c',
  м: 'v',
  и: 'b',
  т: 'n',
  ь: 'm',
  б: ',',
  ю: '.'
};

// Punctuation the Russian layout places on other keys, by the US character
// produced by the same key and shift state.
const RUSSIAN_PUNCTUATION: Record<string, string> = {
  '"': '@',
  '№': '#',
  ';': '$',
  ':': '^',
  '?': '&',
  '/': '|',
  '.': '/',
  ',': '?'
};

function russianLetterStep(character: string): KeyStep | null {
  const lower = character.toLowerCase();
  const key = RUSSIAN_LETTERS[lower];
  if (!key) return null;
  const base = baseStep(key);
  if (!base) return null;
  // Upper case is Shift on the same key; the US character at that key (such
  // as '[' for Х) has no case of its own.
  return { modifier: character === lower ? 0 : SHIFT, usage: base.usage };
}

function isLatinLetter(character: string): boolean {
  const lower = character.toLowerCase();
  return lower >= 'a' && lower <= 'z' && lower.length === 1;
}

const defaultLayoutState: TextLayoutState = { script: 'latin' };

export function textToKeyReports(
  text: string,
  state: TextLayoutState = defaultLayoutState
): KeyStep[] {
  const steps: KeyStep[] = [];
  for (const character of text) {
    let step: KeyStep | null;
    const russian = russianLetterStep(character);
    if (russian) {
      state.script = 'cyrillic';
      step = russian;
    } else if (isLatinLetter(character)) {
      state.script = 'latin';
      step = baseStep(character);
    } else if (state.script === 'cyrillic' && character in RUSSIAN_PUNCTUATION) {
      step = baseStep(RUSSIAN_PUNCTUATION[character]);
    } else {
      step = baseStep(character);
    }
    if (step) {
      steps.push(step);
    }
  }
  return steps;
}
