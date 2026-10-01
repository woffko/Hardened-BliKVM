import { useCallback, useEffect, useRef } from 'react';
import { getDefaultStore, useAtomValue } from 'jotai';

import type { HidReport } from '@/api/restricted-control.ts';
import { supportsRestrictedControls } from '@/lib/capabilities.ts';
import { ASSISTANT_ACTIVE, HidLeaseSession, httpsLease } from '@/lib/hid-lease.ts';
import { MouseQueue } from '@/lib/hid-report-queue.ts';
import { quicLease } from '@/lib/quic/lease.ts';
import { quicSession } from '@/lib/quic/session.ts';
import { ensureFocus, resetFocusConfirmation } from '@/lib/restricted-power.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { leaseTransportAtom } from '@/jotai/transport.ts';
import { useFocusConfirmation } from '@/hooks/useFocusConfirmation.ts';

const LEASE_TTL_SECONDS = 30;
const MOUSE_FLUSH_MS = 10;

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

function cancelSharedSession() {
  queueGeneration += 1;
  mouseQueue.clear();
  const sessions = new Set([sharedSession, startingSession]);
  sharedSession = null;
  startingSession = null;
  getDefaultStore().set(leaseTransportAtom, null);
  sharedQueue = Promise.resolve();
  for (const session of sessions) {
    if (session) void session.stop().catch(() => undefined);
  }
}

export function useHidLease() {
  const manifest = useAtomValue(capabilitiesAtom);
  const restricted = supportsRestrictedControls(manifest);
  const { confirmed, ensureConfirmed, disableControl } = useFocusConfirmation();
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
      const stopControl = () => {
        disableControl();
        cancelSharedSession();
      };
      const visibilityChanged = () => {
        if (document.visibilityState === 'hidden') stopControl();
      };
      window.addEventListener('blur', stopControl);
      document.addEventListener('visibilitychange', visibilityChanged);
      lifecycleCleanup = () => {
        window.removeEventListener('blur', stopControl);
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
      const current = manifestRef.current;
      if (!current) return null;
      await ensureFocus(current.platform.id);
      if (generation !== queueGeneration) return null;

      if (!sharedSession) {
        const platformId = current.platform.id;
        // The transport is chosen now, when control is enabled: the QUIC
        // session if one is open, HTTPS otherwise. A session that is lost
        // fails the lease's requests and turns control off, like any failure;
        // enabling it again chooses anew.
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
      if (session) await session.report(report);
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
          disableControl();
          cancelSharedSession();
        });
      return sharedQueue;
    },
    [disableControl]
  );

  const enqueue = useCallback(
    (report: HidReport) => schedule((generation) => send(report, generation)),
    [schedule, send]
  );
  enqueueRef = enqueue;

  const reportHid = useCallback(
    (report: HidReport) => {
      if (!manifestRef.current || !confirmedRef.current) return;
      if (report.mode === 'keyboard') {
        mouseQueue.flush();
        void enqueue(report);
      } else {
        mouseQueue.push(report);
      }
    },
    [enqueue]
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

  return { restricted, reportHid, release, hold };
}
