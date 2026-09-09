import { useEffect, useRef, useState } from 'react';
import type {
  ApplicationActivationIntent,
  ApplicationActivationReceipt,
  ApplicationSlot,
  ApplicationUpdateStatus
} from '@/types/application-update.ts';

import { activateApplication } from '@/api/application-update.ts';
import {
  currentApplicationIntent,
  eligibleApplicationTarget,
  isApplicationActivationReceipt,
  reconcileApplicationActivation
} from '@/lib/application-update.ts';

type Phase = 'idle' | 'confirming' | 'sending' | 'awaiting' | 'uncertain' | 'rejected';
type Attempt = {
  phase: Phase;
  intent: ApplicationActivationIntent | null;
  receipt: ApplicationActivationReceipt | null;
};

export function useApplicationActivation(
  enabled: boolean,
  ready: boolean,
  status: ApplicationUpdateStatus | null,
  target: ApplicationSlot | null,
  refresh: () => void
) {
  const [attempt, setAttempt] = useState<Attempt>({ phase: 'idle', intent: null, receipt: null });
  const active = useRef<AbortController | null>(null);
  const phase =
    (attempt.phase === 'awaiting' || attempt.phase === 'uncertain') &&
    attempt.intent &&
    ready &&
    status
      ? reconcileApplicationActivation(attempt.intent, attempt.receipt, status)
      : attempt.phase;
  const busy = phase === 'sending' || phase === 'awaiting';
  const canReview =
    enabled &&
    ready &&
    !!status &&
    !!target &&
    eligibleApplicationTarget(target, status) &&
    !busy &&
    !active.current;
  const canConfirm =
    enabled &&
    ready &&
    !!status &&
    !!attempt.intent &&
    !!target &&
    target.slotId === attempt.intent.target.slotId &&
    currentApplicationIntent(attempt.intent, status);
  useEffect(
    () => () => {
      active.current?.abort();
      active.current = null;
    },
    []
  );
  useEffect(() => {
    if (!enabled) {
      active.current?.abort();
      active.current = null;
      setAttempt((value) =>
        value.phase === 'sending'
          ? { ...value, phase: 'uncertain' }
          : value.phase === 'confirming'
            ? { ...value, phase: 'idle' }
            : value
      );
    }
  }, [enabled]);
  const review = () => {
    if (!canReview || !target || !status) return;
    setAttempt({
      phase: 'confirming',
      receipt: null,
      intent: {
        target: { ...target },
        current: { ...status.confirmed },
        expectedGeneration: status.generation
      }
    });
    refresh();
  };
  const close = () => {
    if (!active.current) setAttempt((value) => ({ ...value, phase: 'idle' }));
  };
  const confirm = async () => {
    if (attempt.phase !== 'confirming' || !canConfirm || active.current || !attempt.intent) return;
    const intent = attempt.intent;
    const controller = new AbortController();
    active.current = controller;
    setAttempt({ phase: 'sending', intent, receipt: null });
    try {
      const response = await activateApplication(
        intent.target.slotId,
        intent.expectedGeneration,
        controller.signal
      );
      if (active.current !== controller) return;
      if (
        response.code === 0 &&
        isApplicationActivationReceipt(response.data, intent.target.slotId)
      ) {
        setAttempt({ phase: 'awaiting', intent, receipt: response.data });
      } else {
        setAttempt({
          phase: response.code === -1 || response.code === -409 ? 'rejected' : 'uncertain',
          intent,
          receipt: null
        });
      }
    } catch {
      if (active.current === controller) setAttempt({ phase: 'uncertain', intent, receipt: null });
    } finally {
      if (active.current === controller) {
        active.current = null;
        refresh();
      }
    }
  };
  return {
    ...attempt,
    phase,
    canReview,
    canConfirm,
    review,
    close,
    confirm,
    busy
  };
}
