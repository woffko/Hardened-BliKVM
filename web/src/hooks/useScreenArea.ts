import { useEffect, useRef, useState } from 'react';
import { useAtom, useAtomValue } from 'jotai';

import { getScreenArea, setScreenArea } from '@/api/hid.ts';
import { afterLayoutWrite, isScreenArea, layoutStep } from '@/lib/screen-area.ts';
import { screenAreaAtom } from '@/jotai/mouse.ts';
import { agentScreenAreaAtom } from '@/jotai/target-agent.ts';

/**
 * Where the captured screen lies in the target's desktop, as the board keeps
 * it: the absolute mouse maps into it (docs/design/screen-area-v1.md). A
 * layout the target agent's monitors make is kept on the board too, so the
 * mouse stays right once the agent is gone; each one is tried once, again
 * after the agent came back or the write failed (`layoutStep`).
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
    if (!enabled || !read) return;
    const step = layoutStep(agent, kept, tried.current);
    tried.current = step.tried;
    if (!step.write) return;
    const key = step.tried ?? '';
    const answered = (applied: boolean) => {
      tried.current = afterLayoutWrite(tried.current, key, applied);
    };
    setScreenArea(step.write.area)
      .then((rsp) => {
        answered(rsp.code === 0);
        if (rsp.code === 0) setKept(isScreenArea(rsp.data?.area) ? rsp.data.area : null);
      })
      .catch(() => answered(false));
  }, [enabled, read, agent, kept, setKept]);
}
