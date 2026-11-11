import { useEffect, useState } from 'react';

import { getCaseLeds, type CaseLeds } from '@/api/atx.ts';

const READ_EVERY_MS = 1000;

/**
 * The target's case LEDs as the board reads them, asked once a second while
 * the page is shown and not while it is hidden
 * (docs/baselines/work-plan-20261111-hdd-indicator.md). Null until the board
 * has answered, and after a read that failed.
 */
export function useCaseLeds(enabled: boolean): CaseLeds | null {
  const [leds, setLeds] = useState<CaseLeds | null>(null);

  useEffect(() => {
    if (!enabled) return;
    let gone = false;
    let asking = false;
    const read = async () => {
      if (asking || document.visibilityState === 'hidden') return;
      asking = true;
      try {
        const rsp = await getCaseLeds();
        if (!gone) setLeds(rsp.code === 0 ? rsp.data : null);
      } catch {
        if (!gone) setLeds(null);
      } finally {
        asking = false;
      }
    };
    void read();
    const timer = window.setInterval(() => void read(), READ_EVERY_MS);
    const shown = () => {
      if (document.visibilityState === 'visible') void read();
    };
    document.addEventListener('visibilitychange', shown);
    return () => {
      gone = true;
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', shown);
    };
  }, [enabled]);

  return leds;
}
