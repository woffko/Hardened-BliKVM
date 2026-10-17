import { useEffect, useState } from 'react';
import { getDefaultStore, useAtomValue } from 'jotai';

import * as api from '@/api/target-agent.ts';
import { agentKeyboardAtom, agentLayoutSyncAtom, targetAgentAtom } from '@/jotai/target-agent.ts';
import { useFocusConfirmation } from '@/hooks/useFocusConfirmation.ts';
import { useHidLease } from '@/hooks/useHidLease.ts';

/** Whether a layout's language tag is `language` ("ru" for "ru-RU"). */
export function speaks(tag: string, language: string): boolean {
  return tag.toLowerCase() === language || tag.toLowerCase().startsWith(`${language}-`);
}

// The switches one after another, each decided when its turn comes: a
// language chosen while the last switch is still on its way is compared
// with the layout that switch gave.
let switching: Promise<void> = Promise.resolve();

async function switchTo(language: string, hold: () => void, drained: () => Promise<void>) {
  const store = getDefaultStore();
  const keyboard = store.get(agentKeyboardAtom);
  if (!keyboard || (keyboard.current && speaks(keyboard.current.language, language))) return;
  const wanted = keyboard.installed.find((layout) => speaks(layout.language, language));
  if (!wanted) return;
  // Under the keyboard's and mouse's lease, as every call to the agent is:
  // taken without input if no key took it yet.
  hold();
  await drained();
  await api.askTargetAgent({ set_layout: { id: wanted.id } });
  const latest = store.get(agentKeyboardAtom);
  if (latest) store.set(agentKeyboardAtom, { ...latest, current: wanted });
}

/**
 * Switches the target's layout to the on-screen keyboard's language while
 * the operator asked for it, the agent is connected and remote control is
 * on: as the keyboard opens, as its language is chosen, as the layouts are
 * first read and as control is taken, the window in front is switched to
 * the first installed layout of that language unless its layout speaks it.
 * A layout switched otherwise later (in the Keyboard menu, by the phone's
 * keyboard) is left alone. Returns why the last switch failed, or nothing:
 * a failed switch is asked again when control is taken again.
 */
export function useAgentLayoutSync(language: string, open: boolean): string {
  const sync = useAtomValue(agentLayoutSyncAtom);
  const connected = Boolean(useAtomValue(targetAgentAtom)?.agent);
  const read = useAtomValue(agentKeyboardAtom) !== null;
  const { confirmed } = useFocusConfirmation();
  const { hold, drained } = useHidLease();
  const [failure, setFailure] = useState('');

  useEffect(() => {
    if (!open || !sync || !connected || !read || !confirmed) return;
    switching = switching
      .then(() => switchTo(language, hold, drained))
      .then(() => setFailure(''))
      .catch((err: unknown) => setFailure(err instanceof Error ? err.message : String(err)));
  }, [confirmed, connected, drained, hold, language, open, read, sync]);

  return failure;
}
