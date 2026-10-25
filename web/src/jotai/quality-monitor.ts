import { atom } from 'jotai';

import { loadLayout, saveLayout, type QualityMonitorLayout } from '@/lib/quality-monitor.ts';

const layoutAtom = atom<QualityMonitorLayout>(loadLayout());

/** The quality monitor's window; every change is kept in this browser. */
export const qualityMonitorAtom = atom(
  (get) => get(layoutAtom),
  (get, set, update: Partial<QualityMonitorLayout>) => {
    const next = { ...get(layoutAtom), ...update };
    set(layoutAtom, next);
    saveLayout(next);
  }
);
