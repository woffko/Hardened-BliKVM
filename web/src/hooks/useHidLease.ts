import { useCallback, useEffect, useRef } from 'react';
import { message } from 'antd';
import { getDefaultStore, useAtomValue } from 'jotai';
import { useTranslation } from 'react-i18next';

import type { HidReport } from '@/api/restricted-control.ts';
import { supportsRestrictedControls } from '@/lib/capabilities.ts';
import { ASSISTANT_ACTIVE, HidLeaseSession, httpsLease, LeaseNotGranted } from '@/lib/hid-lease.ts';
import { MouseQueue } from '@/lib/hid-report-queue.ts';
import { quicLease } from '@/lib/quic/lease.ts';
import { quicSession } from '@/lib/quic/session.ts';
import { ensureFocus, resetFocusConfirmation } from '@/lib/restricted-power.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { leaseEndedAtom } from '@/jotai/restricted-control.ts';
import { leaseTransportAtom } from '@/jotai/transport.ts';
import { useFocusConfirmation } from '@/hooks/useFocusConfirmation.ts';

const LEASE_TTL_SECONDS = 30;
const MOUSE_FLUSH_MS = 10;
/** After a lease that could not be taken again, input waits this long. */
const RETRY_AFTER_MS = 1000;

/**
 * Every input surface shares one broker lease and one ordered queue. Queue
 * generations prevent reports captured before stop/blur from reaching a new
 * lease, including operations that were already waiting on HTTP.
 */
let sharedSession: HidLeaseSession | null = null;
let startingSession: HidLeaseSession | null = null;
let sharedQueue: Promise<void> = Promise.resolve();
let queueGeneration = 0;
let enqueueRef: ((report: HidReport) => Promise<void>) | null = null;
// At most one mouse report in flight; see MouseQueue.
const mouseQueue = new MouseQueue(
  (report) => (enqueueRef ? enqueueRef(report) : Promise.resolve()),
  MOUSE_FLUSH_MS
);
let consumers = 0;
let lifecycleCleanup: (() => void) | null = null;
let retryAt = 0;

/** Gives the lease back and ends the reports queued for it. */
function cancelSharedSession() {
  queueGeneration += 1;
  mouseQueue.clear();
  sharedQueue = Promise.resolve();
  dropSharedSession();
}

/** Gives the lease back; the reports queued after it take a new one. */
function dropSharedSession() {
  const sessions = new Set([sharedSession, startingSession]);
  const held = sharedSession !== null || startingSession !== null;
  sharedSession = null;
  startingSession = null;
  getDefaultStore().set(leaseTransportAtom, null);
  for (const session of sessions) {
    if (session) void session.stop().catch(() => undefined);
  }
  if (held) {
    const store = getDefaultStore();
    store.set(leaseEndedAtom, store.get(leaseEndedAtom) + 1);
  }
}

/** A key, a button or a touch pressed: what may ask for control. */
function isPress(report: HidReport): boolean {
  if (report.mode === 'keyboard') return report.data.some((byte) => byte !== 0);
  return (report.data[0] ?? 0) !== 0;
}

export function useHidLease() {
  const { t } = useTranslation();
  const manifest = useAtomValue(capabilitiesAtom);
  const restricted = supportsRestrictedControls(manifest);
  const { confirmed, ensureConfirmed, requestConfirmation, disableControl } =
    useFocusConfirmation();
  const manifestRef = useRef(manifest);
  const confirmedRef = useRef(confirmed);
  manifestRef.current = manifest;
  confirmedRef.current = confirmed;

  useEffect(() => {
    if (!confirmed) cancelSharedSession();
  }, [confirmed]);

  useEffect(() => {
    consumers += 1;
    if (consumers === 1) {
      // A window without focus gives the lease back, so the target gets
      // nothing meant for another window; the confirmation stays, and the
      // next input takes the lease again.
      const releaseLease = () => cancelSharedSession();
      const visibilityChanged = () => {
        if (document.visibilityState === 'hidden') releaseLease();
      };
      window.addEventListener('blur', releaseLease);
      document.addEventListener('visibilitychange', visibilityChanged);
      lifecycleCleanup = () => {
        window.removeEventListener('blur', releaseLease);
        document.removeEventListener('visibilitychange', visibilityChanged);
      };
    }
    return () => {
      consumers -= 1;
      if (consumers === 0) {
        lifecycleCleanup?.();
        lifecycleCleanup = null;
        enqueueRef = null;
        disableControl();
        cancelSharedSession();
      }
    };
  }, [disableControl]);

  /** Starts the shared lease if needed; false when it cannot be used. */
  const ensureSession = useCallback(
    async (generation: number): Promise<HidLeaseSession | null> => {
      if (generation !== queueGeneration || !(await ensureConfirmed())) return null;
      if (!sharedSession && performance.now() < retryAt) return null;
      const current = manifestRef.current;
      if (!current) return null;
      await ensureFocus(current.platform.id);
      if (generation !== queueGeneration) return null;

      if (!sharedSession) {
        const platformId = current.platform.id;
        // The transport is chosen when the lease is taken: the QUIC session
        // if one is open, HTTPS otherwise. A lease lost on the way is taken
        // again by the next report, which chooses anew.
        const overQuic = quicSession.isOpen();
        const session = new HidLeaseSession(
          () => ensureFocus(platformId),
          overQuic ? quicLease : httpsLease
        );
        startingSession = session;
        try {
          await session.start(LEASE_TTL_SECONDS);
        } finally {
          if (startingSession === session) startingSession = null;
        }
        if (generation !== queueGeneration) {
          await session.stop().catch(() => undefined);
          return null;
        }
        sharedSession = session;
        getDefaultStore().set(leaseTransportAtom, overQuic ? 'quic' : 'https');
      }
      return generation === queueGeneration ? sharedSession : null;
    },
    [ensureConfirmed]
  );

  const send = useCallback(
    async (report: HidReport, generation: number) => {
      const session = await ensureSession(generation);
      if (!session) return;
      try {
        await session.report(report);
      } catch (error) {
        if (
          generation !== queueGeneration ||
          (error instanceof Error && error.message === ASSISTANT_ACTIVE) ||
          (error instanceof DOMException && error.name === 'AbortError')
        ) {
          throw error;
        }
        // The lease ended under the page: a QUIC session that reconnected, a
        // socket opened again, the board. It is taken again once, and the
        // report is sent with it, so a key is not lost on the way.
        dropSharedSession();
        resetFocusConfirmation();
        const again = await ensureSession(generation);
        if (again) await again.report(report);
      }
    },
    [ensureSession]
  );

  const schedule = useCallback(
    (run: (generation: number) => Promise<unknown>): Promise<void> => {
      const generation = queueGeneration;
      sharedQueue = sharedQueue
        .then(() => run(generation))
        .then(() => undefined)
        .catch((error: unknown) => {
          if (generation !== queueGeneration) return;
          // Held back while the AI assistant has control; the lease is live.
          if (error instanceof Error && error.message === ASSISTANT_ACTIVE) return;
          resetFocusConfirmation();
          if (error instanceof LeaseNotGranted) {
            // The board refused control: the next press asks again.
            disableControl();
            cancelSharedSession();
            void message.warning(
              error.message === 'lease_conflict'
                ? t('control.otherSession')
                : t('control.refused', { reason: error.message })
            );
            return;
          }
          // Anything else passes: the confirmation stays, and input takes
          // the lease again after a moment.
          retryAt = performance.now() + RETRY_AFTER_MS;
          cancelSharedSession();
        });
      return sharedQueue;
    },
    [disableControl, t]
  );

  const enqueue = useCallback(
    (report: HidReport) => schedule((generation) => send(report, generation)),
    [schedule, send]
  );
  enqueueRef = enqueue;

  const reportHid = useCallback(
    (report: HidReport) => {
      if (!manifestRef.current) return;
      if (!confirmedRef.current) {
        // The first press asks for control; it does not reach the target.
        if (isPress(report)) void requestConfirmation();
        return;
      }
      if (report.mode === 'keyboard') {
        mouseQueue.flush();
        void enqueue(report);
      } else {
        mouseQueue.push(report);
      }
    },
    [enqueue, requestConfirmation]
  );

  const release = useCallback(() => {
    disableControl();
    cancelSharedSession();
  }, [disableControl]);

  /**
   * Holds the lease without sending input, so the AI assistant can act
   * through this operator's control while its session is connected.
   */
  const hold = useCallback(() => {
    if (!manifestRef.current || !confirmedRef.current) return;
    schedule((generation) => ensureSession(generation));
  }, [ensureSession, schedule]);

  /** Resolves once the reports queued so far are sent, or dropped. */
  const drained = useCallback(() => sharedQueue.then(() => undefined), []);

  return { restricted, confirmed, reportHid, release, hold, drained };
}
