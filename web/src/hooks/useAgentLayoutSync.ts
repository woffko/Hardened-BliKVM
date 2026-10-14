import { useEffect } from 'react';
import { useAtom, useAtomValue } from 'jotai';

import * as api from '@/api/target-agent.ts';
import { agentKeyboardAtom, agentLayoutSyncAtom, targetAgentAtom } from '@/jotai/target-agent.ts';

/** Whether a layout's language tag is `language` ("ru" for "ru-RU"). */
export function speaks(tag: string, language: string): boolean {
  return tag.toLowerCase() === language || tag.toLowerCase().startsWith(`${language}-`);
}

/**
 * Keeps the target's layout in step with the on-screen keyboard's language
 * while the operator asked for it and the agent is connected: when they
 * differ, the window in front is switched to the first installed layout of
 * that language.
 */
export function useAgentLayoutSync(language: string) {
  const sync = useAtomValue(agentLayoutSyncAtom);
  const connected = Boolean(useAtomValue(targetAgentAtom)?.agent);
  const [keyboard, setKeyboard] = useAtom(agentKeyboardAtom);

  useEffect(() => {
    if (!sync || !connected || !keyboard) return;
    if (keyboard.current && speaks(keyboard.current.language, language)) return;
    const wanted = keyboard.installed.find((layout) => speaks(layout.language, language));
    if (!wanted) return;
    let gone = false;
    api
      .askTargetAgent({ set_layout: { id: wanted.id } })
      .then(() => api.askTargetAgent('keyboard'))
      .then((answer) => {
        if (!gone && typeof answer === 'object' && 'keyboard' in answer)
          setKeyboard(answer.keyboard);
      })
      .catch(() => undefined);
    return () => {
      gone = true;
    };
  }, [connected, keyboard, language, setKeyboard, sync]);
}
