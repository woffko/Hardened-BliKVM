import { useCallback, useEffect, useRef, useState } from 'react';
import type { DiagnosticsView } from '@/types/diagnostics.ts';

import { getDiagnostics } from '@/api/diagnostics.ts';
import { isDiagnosticsView } from '@/lib/diagnostics.ts';

export const DIAGNOSTICS_REFRESH_MS = 10_000;

export type DiagnosticsLoadState = 'loading' | 'ready' | 'stale' | 'error';

type DiagnosticsSnapshot = {
  data: DiagnosticsView | null;
  state: DiagnosticsLoadState;
  lastUpdated: number | null;
  refreshing: boolean;
};

const INITIAL_SNAPSHOT: DiagnosticsSnapshot = {
  data: null,
  state: 'loading',
  lastUpdated: null,
  refreshing: false
};

function isDocumentVisible() {
  return document.visibilityState !== 'hidden';
}

export function useDiagnostics() {
  const [snapshot, setSnapshot] = useState<DiagnosticsSnapshot>(INITIAL_SNAPSHOT);
  const controllerRef = useRef<AbortController | null>(null);
  const timeoutRef = useRef<number | null>(null);
  const mountedRef = useRef(false);

  const clearScheduledRefresh = useCallback(() => {
    if (timeoutRef.current !== null) {
      window.clearTimeout(timeoutRef.current);
      timeoutRef.current = null;
    }
  }, []);

  const load = useCallback(async (): Promise<void> => {
    if (!mountedRef.current || controllerRef.current || !isDocumentVisible()) {
      return;
    }

    clearScheduledRefresh();
    const controller = new AbortController();
    controllerRef.current = controller;
    setSnapshot((current) => ({ ...current, refreshing: true }));

    try {
      const response = await getDiagnostics(controller.signal);
      if (!mountedRef.current || controller.signal.aborted) return;
      if (response.code !== 0 || !isDiagnosticsView(response.data)) {
        throw new Error('invalid diagnostics response');
      }
      setSnapshot({
        data: response.data,
        state: 'ready',
        lastUpdated: Date.now(),
        refreshing: false
      });
    } catch {
      if (!mountedRef.current || controller.signal.aborted) return;
      setSnapshot((current) => ({
        ...current,
        state: current.data ? 'stale' : 'error',
        refreshing: false
      }));
    } finally {
      if (controllerRef.current === controller) controllerRef.current = null;
      if (mountedRef.current && isDocumentVisible()) {
        timeoutRef.current = window.setTimeout(() => void load(), DIAGNOSTICS_REFRESH_MS);
      }
    }
  }, [clearScheduledRefresh]);

  useEffect(() => {
    mountedRef.current = true;
    const onVisibilityChange = () => {
      clearScheduledRefresh();
      if (document.visibilityState === 'hidden') {
        controllerRef.current?.abort();
        setSnapshot((current) =>
          current.data ? { ...current, state: 'stale', refreshing: false } : current
        );
      } else {
        void load();
      }
    };

    document.addEventListener('visibilitychange', onVisibilityChange);
    void load();
    return () => {
      mountedRef.current = false;
      clearScheduledRefresh();
      controllerRef.current?.abort();
      controllerRef.current = null;
      document.removeEventListener('visibilitychange', onVisibilityChange);
    };
  }, [clearScheduledRefresh, load]);

  return {
    ...snapshot,
    refresh: load
  };
}
