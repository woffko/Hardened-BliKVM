/**
 * Text typed from the page, kept readable on the target with the agent's
 * help (docs/design/target-agent-v1.md): the page types Latin letters at
 * the US layout's positions and Cyrillic ones at the Russian layout's, so
 * the target's layout in front must match the letters. When the operator
 * asks for it, the page cuts the text into runs of one script and, before a
 * run whose layout is not the target's, asks the agent to switch to an
 * installed layout of that script: English for Latin letters, Russian for
 * Cyrillic ones.
 */
export type Script = 'latin' | 'cyrillic';

export type TypedLayout = { id: string; language: string; name: string };

const CYRILLIC = /[а-яё]/i;
const LATIN = /[a-z]/i;

export function scriptOf(character: string): Script | null {
  if (LATIN.test(character)) return 'latin';
  if (CYRILLIC.test(character)) return 'cyrillic';
  return null;
}

/**
 * The text in runs of one script. What comes before the first letter goes
 * with it; what follows a letter goes with that letter's run until a letter
 * of the other script.
 */
export function scriptRuns(text: string): { script: Script | null; text: string }[] {
  const runs: { script: Script | null; text: string }[] = [];
  let current: { script: Script | null; text: string } = { script: null, text: '' };
  for (const character of text) {
    const script = scriptOf(character);
    if (script && current.script && script !== current.script) {
      runs.push(current);
      current = { script, text: '' };
    } else if (script && !current.script) {
      current.script = script;
    }
    current.text += character;
  }
  if (current.text) runs.push(current);
  return runs;
}

/** The installed layout the page types a script for, if there is one. */
export function layoutForScript<Layout extends TypedLayout>(
  installed: Layout[],
  script: Script
): Layout | null {
  const language = script === 'latin' ? 'en' : 'ru';
  const speaks = (layout: TypedLayout) =>
    layout.language.toLowerCase() === language ||
    layout.language.toLowerCase().startsWith(`${language}-`);
  const exact = script === 'latin' ? 'en-us' : 'ru-ru';
  return (
    installed.find((layout) => layout.language.toLowerCase() === exact) ??
    installed.find(speaks) ??
    null
  );
}
