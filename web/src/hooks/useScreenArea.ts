import { useEffect, useRef, useState } from 'react';
import { useAtom, useAtomValue } from 'jotai';

import { getScreenArea, setScreenArea } from '@/api/hid.ts';
import { isScreenArea, sameScreenArea } from '@/lib/screen-area.ts';
import { screenAreaAtom } from '@/jotai/mouse.ts';
import { agentScreenAreaAtom } from '@/jotai/target-agent.ts';

/**
 * Where the captured screen lies in the target's desktop, as the board keeps
 * it: the absolute mouse maps into it (docs/design/screen-area-v1.md). A
 * layout the target agent's monitors make is kept on the board too, so the
 * mouse stays right once the agent is gone; each one is tried once.
 */
export function useScreenArea(enabled: boolean) {
  const [kept, setKept] = useAtom(screenAreaAtom);
  const agent = useAtomValue(agentScreenAreaAtom);
  const [read, setRead] = useState(false);
  const tried = useRef<string | null>(null);

  useEffect(() => {
    if (!enabled) return;
    let gone = false;
    getScreenArea()
      .then((rsp) => {
        if (gone || rsp.code !== 0) return;
        setKept(isScreenArea(rsp.data?.area) ? rsp.data.area : null);
        setRead(true);
      })
      .catch(() => undefined);
    return () => {
      gone = true;
    };
  }, [enabled, setKept]);

  useEffect(() => {
    if (!enabled || !read || agent === null) return;
    const wanted = agent === 'whole' ? null : agent;
    if (sameScreenArea(wanted, kept)) return;
    const key = JSON.stringify(wanted);
    if (tried.current === key) return;
    tried.current = key;
    setScreenArea(wanted)
      .then((rsp) => {
        if (rsp.code === 0) setKept(isScreenArea(rsp.data?.area) ? rsp.data.area : null);
      })
      .catch(() => undefined);
  }, [enabled, read, agent, kept, setKept]);
}
