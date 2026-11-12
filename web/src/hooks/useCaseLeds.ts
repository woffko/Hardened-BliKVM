import { useEffect, useState } from 'react';

import { getCaseLeds, type CaseLeds } from '@/api/atx.ts';
import { getBaseUrl } from '@/lib/service.ts';

/** How often the light route is read while the socket is down. */
const READ_EVERY_MS = 1000;
/** How long after a socket went down a new one is opened. */
const OPEN_AGAIN_MS = 2000;

/**
 * The target's case LEDs as the board reads them, as KVMD shows them: while
 * the page is shown it listens to `/api/atx/leds/socket`, which sends the LEDs
 * when it opens and at each change (boardd holds a change 100 ms); hidden, it
 * closes the socket. While the socket is down, the page reads `/api/atx/leds`
 * once a second and opens a new socket after 2 s
 * (docs/baselines/work-plan-20261111-hdd-indicator.md, H7). Null until the
 * board has answered, and after a read that failed.
 */
export function useCaseLeds(enabled: boolean): CaseLeds | null {
  const [leds, setLeds] = useState<CaseLeds | null>(null);

  useEffect(() => {
    if (!enabled) return;
    let gone = false;
    let socket: WebSocket | null = null;
    let reopen: number | undefined;
    let reads: number | undefined;
    let asking = false;

    const read = async () => {
      if (asking || socket?.readyState === WebSocket.OPEN) return;
      asking = true;
      try {
        const rsp = await getCaseLeds();
        if (!gone && socket?.readyState !== WebSocket.OPEN) {
          setLeds(rsp.code === 0 ? rsp.data : null);
        }
      } catch {
        if (!gone) setLeds(null);
      } finally {
        asking = false;
      }
    };
    const stopReads = () => {
      window.clearInterval(reads);
      reads = undefined;
    };
    const startReads = () => {
      if (reads !== undefined) return;
      void read();
      reads = window.setInterval(() => void read(), READ_EVERY_MS);
    };

    const open = () => {
      if (gone || socket || document.visibilityState === 'hidden') return;
      const ws = new WebSocket(`${getBaseUrl('ws')}/api/atx/leds/socket`);
      socket = ws;
      ws.onopen = stopReads;
      ws.onmessage = (event: MessageEvent) => {
        if (gone || socket !== ws || typeof event.data !== 'string') return;
        try {
          const data = JSON.parse(event.data) as Partial<CaseLeds>;
          setLeds({ powerLed: data.powerLed ?? null, hddLed: data.hddLed ?? null });
        } catch {
          // Not the board's answer: the last one stands.
        }
      };
      ws.onclose = () => {
        // A socket let go of when the page was hidden ends here.
        if (socket !== ws) return;
        socket = null;
        if (gone || document.visibilityState === 'hidden') return;
        startReads();
        window.clearTimeout(reopen);
        reopen = window.setTimeout(() => {
          reopen = undefined;
          open();
        }, OPEN_AGAIN_MS);
      };
    };
    const close = () => {
      window.clearTimeout(reopen);
      reopen = undefined;
      stopReads();
      const ws = socket;
      socket = null;
      ws?.close();
    };
    const visibility = () => {
      if (document.visibilityState === 'hidden') close();
      else open();
    };

    open();
    document.addEventListener('visibilitychange', visibility);
    return () => {
      gone = true;
      document.removeEventListener('visibilitychange', visibility);
      close();
    };
  }, [enabled]);

  return leds;
}
