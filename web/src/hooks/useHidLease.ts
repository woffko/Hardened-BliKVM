import { useCallback, useEffect, useRef } from 'react';
import { useAtomValue } from 'jotai';

import type { HidReport } from '@/api/restricted-control.ts';
import { supportsRestrictedControls } from '@/lib/capabilities.ts';
import { ASSISTANT_ACTIVE, HidLeaseSession } from '@/lib/hid-lease.ts';
import { hasWheel, mergeMouseReport, type MouseReport } from '@/lib/hid-report-queue.ts';
import { ensureFocus, resetFocusConfirmation } from '@/lib/restricted-power.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
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
let pendingMouse: MouseReport | null = null;
let mouseFlushTimer: ReturnType<typeof setTimeout> | null = null;
let enqueueRef: ((report: HidReport) => void) | null = null;
let consumers = 0;
let lifecycleCleanup: (() => void) | null = null;

function clearPendingMouse() {
  if (mouseFlushTimer !== null) {
    clearTimeout(mouseFlushTimer);
    mouseFlushTimer = null;
  }
  pendingMouse = null;
}

function flushMouse() {
  if (mouseFlushTimer !== null) {
    clearTimeout(mouseFlushTimer);
    mouseFlushTimer = null;
  }
  const report = pendingMouse;
  pendingMouse = null;
  if (report && enqueueRef) enqueueRef(report);
}

function scheduleMouseFlush() {
  if (mouseFlushTimer !== null) return;
  mouseFlushTimer = setTimeout(flushMouse, MOUSE_FLUSH_MS);
}

function queueMouse(report: MouseReport) {
  if (hasWheel(report)) {
    flushMouse();
    enqueueRef?.(report);
    return;
  }
  if (pendingMouse) {
    const merged = mergeMouseReport(pendingMouse, report);
    if (merged) {
      pendingMouse = merged;
      return;
    }
    flushMouse();
  }
  pendingMouse = report;
  scheduleMouseFlush();
}

function cancelSharedSession() {
  queueGeneration += 1;
  clearPendingMouse();
  const sessions = new Set([sharedSession, startingSession]);
  sharedSession = null;
  startingSession = null;
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
        const session = new HidLeaseSession(() => ensureFocus(platformId));
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
    (run: (generation: number) => Promise<unknown>) => {
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
        flushMouse();
        enqueue(report);
      } else {
        queueMouse(report);
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
