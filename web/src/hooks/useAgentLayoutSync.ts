import { useEffect } from 'react';
import { getDefaultStore, useAtomValue } from 'jotai';

import * as api from '@/api/target-agent.ts';
import { agentKeyboardAtom, agentLayoutSyncAtom, targetAgentAtom } from '@/jotai/target-agent.ts';

/** Whether a layout's language tag is `language` ("ru" for "ru-RU"). */
export function speaks(tag: string, language: string): boolean {
  return tag.toLowerCase() === language || tag.toLowerCase().startsWith(`${language}-`);
}

// The switches one after another, each decided when its turn comes: a
// language chosen while the last switch is still on its way is compared
// with the layout that switch gave.
let switching: Promise<void> = Promise.resolve();

async function switchTo(language: string) {
  const store = getDefaultStore();
  const keyboard = store.get(agentKeyboardAtom);
  if (!keyboard || (keyboard.current && speaks(keyboard.current.language, language))) return;
  const wanted = keyboard.installed.find((layout) => speaks(layout.language, language));
  if (!wanted) return;
  await api.askTargetAgent({ set_layout: { id: wanted.id } });
  const latest = store.get(agentKeyboardAtom);
  if (latest) store.set(agentKeyboardAtom, { ...latest, current: wanted });
}

/**
 * Switches the target's layout to the on-screen keyboard's language while
 * the operator asked for it and the agent is connected: as the keyboard
 * opens, as its language is chosen, and as the layouts are first read, the
 * window in front is switched to the first installed layout of that
 * language unless its layout speaks it. A layout switched otherwise later
 * (in the Keyboard menu, by the phone's keyboard) is left alone.
 */
export function useAgentLayoutSync(language: string, open: boolean) {
  const sync = useAtomValue(agentLayoutSyncAtom);
  const connected = Boolean(useAtomValue(targetAgentAtom)?.agent);
  const read = useAtomValue(agentKeyboardAtom) !== null;

  useEffect(() => {
    if (!open || !sync || !connected || !read) return;
    switching = switching.then(() => switchTo(language)).catch(() => undefined);
  }, [connected, language, open, read, sync]);
}
