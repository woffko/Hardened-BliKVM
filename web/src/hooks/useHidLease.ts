import { useCallback, useEffect, useRef } from 'react';
import { useAtomValue } from 'jotai';

import type { HidReport } from '@/api/restricted-control.ts';
import { supportsRestrictedControls } from '@/lib/capabilities.ts';
import { HidLeaseSession } from '@/lib/hid-lease.ts';
import { ensureFocus, resetFocusConfirmation } from '@/lib/restricted-power.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { useFocusConfirmation } from '@/hooks/useFocusConfirmation.ts';

const LEASE_TTL_SECONDS = 30;
const MOUSE_FLUSH_MS = 10;

/**
 * The broker admits a single active HID lease, so the keyboard, the mouse and
 * the mobile trackpad must share one lease and one ordered queue. A lease per
 * surface would make them mutually exclusive with `lease_conflict`.
 *
 * Keyboard reports keep their order. Mouse movement is coalesced to the latest
 * report per flush so a burst of pointer events cannot build an HTTP backlog
 * and make the cursor trail far behind.
 */
let sharedSession: HidLeaseSession | null = null;
let sharedQueue: Promise<void> = Promise.resolve();
let latestMouse: HidReport | null = null;
let mouseFlushTimer: ReturnType<typeof setTimeout> | null = null;
let enqueueRef: ((report: HidReport) => void) | null = null;
let consumers = 0;

function scheduleMouseFlush() {
  if (mouseFlushTimer !== null) {
    return;
  }
  mouseFlushTimer = setTimeout(() => {
    mouseFlushTimer = null;
    const report = latestMouse;
    latestMouse = null;
    if (report && enqueueRef) {
      enqueueRef(report);
    }
  }, MOUSE_FLUSH_MS);
}

export function useHidLease() {
  const manifest = useAtomValue(capabilitiesAtom);
  const restricted = supportsRestrictedControls(manifest);
  const { ensureConfirmed } = useFocusConfirmation();
  const manifestRef = useRef(manifest);
  manifestRef.current = manifest;

  useEffect(() => {
    consumers += 1;
    return () => {
      consumers -= 1;
      if (consumers === 0) {
        if (mouseFlushTimer !== null) {
          clearTimeout(mouseFlushTimer);
          mouseFlushTimer = null;
        }
        latestMouse = null;
        const session = sharedSession;
        sharedSession = null;
        if (session) {
          void session.stop();
        }
      }
    };
  }, []);

  const send = useCallback(
    async (report: HidReport) => {
      if (!(await ensureConfirmed())) {
        return;
      }
      const current = manifestRef.current;
      if (!current) {
        return;
      }
      await ensureFocus(current.platform.id);
      if (!sharedSession) {
        const platformId = current.platform.id;
        const session = new HidLeaseSession(() => ensureFocus(platformId));
        await session.start(LEASE_TTL_SECONDS);
        sharedSession = session;
      }
      await sharedSession.report(report);
    },
    [ensureConfirmed]
  );

  const enqueue = useCallback(
    (report: HidReport) => {
      sharedQueue = sharedQueue
        .then(() => send(report))
        .catch(() => {
          const session = sharedSession;
          sharedSession = null;
          // A refused report may mean the confirmation lapsed, so the next
          // attempt re-confirms instead of waiting for the cache.
          resetFocusConfirmation();
          if (session) {
            void session.stop().catch(() => undefined);
          }
        });
    },
    [send]
  );
  enqueueRef = enqueue;

  const reportHid = useCallback(
    (report: HidReport) => {
      if (!manifestRef.current) {
        return;
      }
      if (report.mode === 'keyboard') {
        enqueue(report);
        return;
      }
      latestMouse = report;
      scheduleMouseFlush();
    },
    [enqueue]
  );

  const release = useCallback(() => {
    if (mouseFlushTimer !== null) {
      clearTimeout(mouseFlushTimer);
      mouseFlushTimer = null;
    }
    latestMouse = null;
    const session = sharedSession;
    sharedSession = null;
    if (session) {
      void session.stop();
    }
  }, []);

  return { restricted, reportHid, release };
}
