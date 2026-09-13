import { useCallback, useEffect, useRef } from 'react';
import { useAtomValue } from 'jotai';

import type { HidReport } from '@/api/restricted-control.ts';
import { supportsRestrictedControls } from '@/lib/capabilities.ts';
import { HidLeaseSession } from '@/lib/hid-lease.ts';
import { ensureFocus } from '@/lib/restricted-power.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';

const LEASE_TTL_SECONDS = 30;

/**
 * Keeps one HID lease for the keyboard surface and serializes reports so the
 * backend sees them in order. Only active when the manifest advertises the
 * restricted HID capability; otherwise the caller keeps the legacy path.
 */
export function useHidLease() {
  const manifest = useAtomValue(capabilitiesAtom);
  const restricted = supportsRestrictedControls(manifest);
  const sessionRef = useRef<HidLeaseSession | null>(null);
  const queueRef = useRef<Promise<void>>(Promise.resolve());

  useEffect(
    () => () => {
      const session = sessionRef.current;
      sessionRef.current = null;
      if (session) {
        void session.stop();
      }
    },
    []
  );

  const reportHid = useCallback(
    (report: HidReport) => {
      if (!manifest) {
        return;
      }
      queueRef.current = queueRef.current
        .then(async () => {
          await ensureFocus(manifest.platform.id);
          if (!sessionRef.current) {
            const session = new HidLeaseSession();
            await session.start(LEASE_TTL_SECONDS);
            sessionRef.current = session;
          }
          await sessionRef.current.report(report);
        })
        .catch(() => {
          sessionRef.current = null;
        });
    },
    [manifest]
  );

  const release = useCallback(() => {
    const session = sessionRef.current;
    sessionRef.current = null;
    if (session) {
      void session.stop();
    }
  }, []);

  return { restricted, reportHid, release };
}
