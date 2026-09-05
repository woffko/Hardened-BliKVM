import { useCallback, useEffect, useState } from 'react';
import type { ApplicationUpdateStatus } from '@/types/application-update.ts';

import { getApplicationUpdateStatus } from '@/api/application-update.ts';
import { isApplicationUpdateStatus } from '@/lib/application-update.ts';

type Snapshot = {
  data: ApplicationUpdateStatus | null;
  state: 'loading' | 'ready' | 'stale' | 'error';
  refreshing: boolean;
};

export function useApplicationUpdateStatus() {
  const [snapshot, setSnapshot] = useState<Snapshot>({
    data: null,
    state: 'loading',
    refreshing: false
  });
  const [generation, setGeneration] = useState(0);
  const [visible, setVisible] = useState(() => document.visibilityState !== 'hidden');
  useEffect(() => {
    const changed = () => setVisible(document.visibilityState !== 'hidden');
    document.addEventListener('visibilitychange', changed);
    return () => document.removeEventListener('visibilitychange', changed);
  }, []);
  useEffect(() => {
    if (!visible) {
      setSnapshot((value) => ({
        ...value,
        state: value.data ? 'stale' : value.state,
        refreshing: false
      }));
      return;
    }
    const controller = new AbortController();
    let active = true;
    let timer: number | undefined;
    setSnapshot((value) => ({ ...value, refreshing: true }));
    const load = async () => {
      try {
        const response = await getApplicationUpdateStatus(controller.signal);
        if (!active) return;
        if (response.code !== 0 || !isApplicationUpdateStatus(response.data))
          throw new Error('invalid update status');
        setSnapshot({ data: response.data, state: 'ready', refreshing: false });
      } catch {
        if (!active) return;
        setSnapshot((value) => ({
          ...value,
          state: value.data ? 'stale' : 'error',
          refreshing: false
        }));
      } finally {
        if (active) timer = window.setTimeout(() => setGeneration((value) => value + 1), 5000);
      }
    };
    void load();
    return () => {
      active = false;
      controller.abort();
      window.clearTimeout(timer);
    };
  }, [visible, generation]);
  const refresh = useCallback(() => setGeneration((value) => value + 1), []);
  return { ...snapshot, refresh };
}
