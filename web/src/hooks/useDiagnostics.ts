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
  const [refreshGeneration, setRefreshGeneration] = useState(0);
  const [visible, setVisible] = useState(isDocumentVisible);
  const controllerRef = useRef<AbortController | null>(null);

  useEffect(() => {
    const onVisibilityChange = () => {
      const nextVisible = isDocumentVisible();
      setVisible(nextVisible);
      if (!nextVisible) {
        controllerRef.current?.abort();
        setSnapshot((current) =>
          current.data ? { ...current, state: 'stale', refreshing: false } : current
        );
      }
    };

    document.addEventListener('visibilitychange', onVisibilityChange);
    return () => {
      controllerRef.current?.abort();
      controllerRef.current = null;
      document.removeEventListener('visibilitychange', onVisibilityChange);
    };
  }, []);

  useEffect(() => {
    if (!visible) return;

    const controller = new AbortController();
    let active = true;
    let timeout: number | null = null;
    controllerRef.current = controller;
    setSnapshot((current) => ({ ...current, refreshing: true }));

    const load = async () => {
      let scheduleNext = false;
      try {
        const response = await getDiagnostics(controller.signal);
        if (!active || controller.signal.aborted) return;
        if (response.code !== 0 || !isDiagnosticsView(response.data)) {
          throw new Error('invalid diagnostics response');
        }
        setSnapshot({
          data: response.data,
          state: 'ready',
          lastUpdated: Date.now(),
          refreshing: false
        });
        scheduleNext = true;
      } catch {
        if (!active || controller.signal.aborted) return;
        setSnapshot((current) => ({
          ...current,
          state: current.data ? 'stale' : 'error',
          refreshing: false
        }));
      } finally {
        if (controllerRef.current === controller) controllerRef.current = null;
        if (active && scheduleNext) {
          timeout = window.setTimeout(
            () => setRefreshGeneration((generation) => generation + 1),
            DIAGNOSTICS_REFRESH_MS
          );
        }
      }
    };

    void load();
    return () => {
      active = false;
      if (timeout !== null) window.clearTimeout(timeout);
      controller.abort();
      if (controllerRef.current === controller) controllerRef.current = null;
    };
  }, [refreshGeneration, visible]);

  const refresh = useCallback(() => {
    setRefreshGeneration((generation) => generation + 1);
  }, []);

  return {
    ...snapshot,
    refresh
  };
}
