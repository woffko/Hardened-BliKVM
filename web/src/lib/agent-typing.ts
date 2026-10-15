/**
 * Text typed from the page, kept readable on the target with the agent's
 * help (docs/design/target-agent-v1.md): the page types Latin letters at
 * the US layout's positions, Cyrillic ones at the Russian layout's and
 * õ ä ö ü š ž at the Estonian layout's (lib/hid-text), so the target's
 * layout in front must match the letters. When the operator asks for it,
 * the page cuts the text into runs of one script and, before a run whose
 * layout is not the target's, asks the agent to switch to an installed
 * layout of that script: English for Latin letters, Russian for Cyrillic
 * ones, Estonian for a run with Estonian letters. The Estonian layout has
 * the Latin letters where the US layout has them, so Latin text keeps an
 * Estonian layout in front, and Latin letters join an Estonian run.
 */
export type Script = 'latin' | 'cyrillic' | 'estonian';

export type TypedLayout = { id: string; language: string; name: string };

const ESTONIAN = /[õäöüšž]/i;
const CYRILLIC = /[а-яё]/i;
const LATIN = /[a-z]/i;

export function scriptOf(character: string): Script | null {
  if (ESTONIAN.test(character)) return 'estonian';
  if (LATIN.test(character)) return 'latin';
  if (CYRILLIC.test(character)) return 'cyrillic';
  return null;
}

/** Whether letters of these scripts may be typed in one layout. */
function together(one: Script, other: Script): boolean {
  return one === other || (one !== 'cyrillic' && other !== 'cyrillic');
}

/**
 * The text in runs of one script. What comes before the first letter goes
 * with it; what follows a letter goes with that letter's run until a letter
 * that cannot be typed in its layout. A run of Latin letters with an
 * Estonian letter is an Estonian run.
 */
export function scriptRuns(text: string): { script: Script | null; text: string }[] {
  const runs: { script: Script | null; text: string }[] = [];
  let current: { script: Script | null; text: string } = { script: null, text: '' };
  for (const character of text) {
    const script = scriptOf(character);
    if (script && current.script && !together(script, current.script)) {
      runs.push(current);
      current = { script, text: '' };
    } else if (script && (!current.script || script === 'estonian')) {
      current.script = script;
    }
    current.text += character;
  }
  if (current.text) runs.push(current);
  return runs;
}

const LANGUAGES: Record<Script, { language: string; exact: string }> = {
  latin: { language: 'en', exact: 'en-us' },
  cyrillic: { language: 'ru', exact: 'ru-ru' },
  estonian: { language: 'et', exact: 'et-ee' }
};

function speaks(layout: TypedLayout, language: string): boolean {
  const tag = layout.language.toLowerCase();
  return tag === language || tag.startsWith(`${language}-`);
}

/** The installed layout the page types a script for, if there is one. */
export function layoutForScript<Layout extends TypedLayout>(
  installed: Layout[],
  script: Script
): Layout | null {
  const { language, exact } = LANGUAGES[script];
  return (
    installed.find((layout) => layout.language.toLowerCase() === exact) ??
    installed.find((layout) => speaks(layout, language)) ??
    null
  );
}

/**
 * The layout a run is typed in: the one in front when its letters are
 * where the run's are (Latin letters in an English or Estonian layout),
 * else the installed layout of the run's script.
 */
export function layoutToType<Layout extends TypedLayout>(
  installed: Layout[],
  current: Layout | null,
  script: Script
): Layout | null {
  if (script === 'latin' && current && (speaks(current, 'en') || speaks(current, 'et')))
    return current;
  return layoutForScript(installed, script);
}

/** The script a layout's punctuation is typed for. */
export function scriptOfLayout(layout: TypedLayout): Script {
  if (speaks(layout, 'ru')) return 'cyrillic';
  if (speaks(layout, 'et')) return 'estonian';
  return 'latin';
}
