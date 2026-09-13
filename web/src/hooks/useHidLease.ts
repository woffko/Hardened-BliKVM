import { useCallback, useEffect, useRef } from 'react';
import { useAtomValue } from 'jotai';

import type { HidReport } from '@/api/restricted-control.ts';
import { supportsRestrictedControls } from '@/lib/capabilities.ts';
import { HidLeaseSession } from '@/lib/hid-lease.ts';
import { ensureFocus } from '@/lib/restricted-power.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { useFocusConfirmation } from '@/hooks/useFocusConfirmation.ts';

const LEASE_TTL_SECONDS = 30;

/**
 * The broker admits a single active HID lease, so the keyboard, the mouse and
 * the mobile trackpad must share one lease and one ordered queue. A lease per
 * surface would make them mutually exclusive with `lease_conflict`.
 */
let sharedSession: HidLeaseSession | null = null;
let sharedQueue: Promise<void> = Promise.resolve();
let consumers = 0;

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
        const session = sharedSession;
        sharedSession = null;
        if (session) {
          void session.stop();
        }
      }
    };
  }, []);

  const reportHid = useCallback(
    (report: HidReport) => {
      if (!manifestRef.current) {
        return;
      }
      sharedQueue = sharedQueue
        .then(async () => {
          if (!(await ensureConfirmed())) {
            return;
          }
          const current = manifestRef.current;
          if (!current) {
            return;
          }
          await ensureFocus(current.platform.id);
          if (!sharedSession) {
            const session = new HidLeaseSession();
            await session.start(LEASE_TTL_SECONDS);
            sharedSession = session;
          }
          await sharedSession.report(report);
        })
        .catch(() => {
          const session = sharedSession;
          sharedSession = null;
          if (session) {
            void session.stop().catch(() => undefined);
          }
        });
    },
    [ensureConfirmed]
  );

  const release = useCallback(() => {
    const session = sharedSession;
    sharedSession = null;
    if (session) {
      void session.stop();
    }
  }, []);

  return { restricted, reportHid, release };
}
