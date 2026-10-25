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

/**
 * The layout the text is typed for, from the last letter that tells:
 * Latin letters at US positions, Cyrillic ones at the Russian layout's,
 * and õ ä ö ü š ž at the Estonian layout's, whose Latin letters are where
 * the US layout has them, so that Latin letters keep it. It selects the
 * punctuation's positions.
 */
export type TextLayoutState = { script: 'latin' | 'cyrillic' | 'estonian' };

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

// The Windows Estonian layout (KBDEST), as Windows itself maps each
// character to a key and its shift state: the keys by their US positions,
// AltGr as the right Alt. Latin letters and digits are where the US layout
// has them. A dead key is followed by a space, which types its own character.
const ESTONIAN: Record<string, KeyStep & { dead?: true }> = {
  '!': { modifier: 0x02, usage: 0x1e },
  '"': { modifier: 0x02, usage: 0x1f },
  '#': { modifier: 0x02, usage: 0x20 },
  $: { modifier: 0x40, usage: 0x21 },
  '%': { modifier: 0x02, usage: 0x22 },
  '&': { modifier: 0x02, usage: 0x23 },
  "'": { modifier: 0x00, usage: 0x31 },
  '(': { modifier: 0x02, usage: 0x25 },
  ')': { modifier: 0x02, usage: 0x26 },
  '*': { modifier: 0x02, usage: 0x31 },
  '+': { modifier: 0x00, usage: 0x2d },
  ',': { modifier: 0x00, usage: 0x36 },
  '-': { modifier: 0x00, usage: 0x38 },
  '.': { modifier: 0x00, usage: 0x37 },
  '/': { modifier: 0x02, usage: 0x24 },
  ':': { modifier: 0x02, usage: 0x37 },
  ';': { modifier: 0x02, usage: 0x36 },
  '<': { modifier: 0x00, usage: 0x64 },
  '=': { modifier: 0x02, usage: 0x27 },
  '>': { modifier: 0x02, usage: 0x64 },
  '?': { modifier: 0x02, usage: 0x2d },
  '@': { modifier: 0x40, usage: 0x1f },
  '[': { modifier: 0x40, usage: 0x25 },
  '\\': { modifier: 0x40, usage: 0x2d },
  ']': { modifier: 0x40, usage: 0x26 },
  '^': { modifier: 0x42, usage: 0x34 },
  _: { modifier: 0x02, usage: 0x38 },
  '`': { modifier: 0x02, usage: 0x2e, dead: true },
  '{': { modifier: 0x40, usage: 0x24 },
  '|': { modifier: 0x40, usage: 0x64 },
  '}': { modifier: 0x40, usage: 0x27 },
  '~': { modifier: 0x02, usage: 0x35, dead: true },
  õ: { modifier: 0x00, usage: 0x30 },
  ä: { modifier: 0x00, usage: 0x34 },
  ö: { modifier: 0x00, usage: 0x33 },
  ü: { modifier: 0x00, usage: 0x2f },
  š: { modifier: 0x40, usage: 0x16 },
  ž: { modifier: 0x40, usage: 0x1d },
  Õ: { modifier: 0x02, usage: 0x30 },
  Ä: { modifier: 0x02, usage: 0x34 },
  Ö: { modifier: 0x02, usage: 0x33 },
  Ü: { modifier: 0x02, usage: 0x2f },
  Š: { modifier: 0x42, usage: 0x16 },
  Ž: { modifier: 0x42, usage: 0x1d },
  '€': { modifier: 0x40, usage: 0x22 },
  '§': { modifier: 0x40, usage: 0x30 },
  '½': { modifier: 0x40, usage: 0x31 },
  '¤': { modifier: 0x02, usage: 0x21 }
};

// Letters only the Estonian layout has among these: õ ä ö ü š ž.
const ESTONIAN_LETTERS = new Set(['õ', 'ä', 'ö', 'ü', 'š', 'ž']);

function isEstonianLetter(character: string): boolean {
  return ESTONIAN_LETTERS.has(character.toLowerCase());
}

/** The keys an Estonian-layout character takes: a dead key then a space. */
function estonianSteps(character: string): KeyStep[] {
  const step = ESTONIAN[character];
  if (!step) return [];
  const key = { modifier: step.modifier, usage: step.usage };
  return step.dead ? [key, { modifier: 0, usage: PLAIN[' '] }] : [key];
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
    } else if (isEstonianLetter(character)) {
      state.script = 'estonian';
      steps.push(...estonianSteps(character));
      continue;
    } else if (isLatinLetter(character)) {
      if (state.script !== 'estonian') state.script = 'latin';
      step = baseStep(character);
    } else if (state.script === 'cyrillic' && character in RUSSIAN_PUNCTUATION) {
      step = baseStep(RUSSIAN_PUNCTUATION[character]);
    } else if (state.script === 'estonian' && character in ESTONIAN) {
      steps.push(...estonianSteps(character));
      continue;
    } else {
      step = baseStep(character);
    }
    if (step) {
      steps.push(step);
    }
  }
  return steps;
}
