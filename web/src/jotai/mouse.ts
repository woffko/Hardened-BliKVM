import { atom } from 'jotai';

import type { ScreenArea } from '@/lib/screen-area.ts';

export type MobilePointerMode = 'touchsync';

// mouse cursor style
export const mouseStyleAtom = atom('cursor-default');

// mouse mode: absolute or relative
export const mouseModeAtom = atom('absolute');

// mobile TouchSync: relative finger movement sent as absolute HID coordinates
export const mobilePointerModeAtom = atom<MobilePointerMode>('touchsync');

// relative pointer sensitivity multiplier for desktop relative mode
export const pointerSensitivityAtom = atom(1);

// mouse scroll direction: -1 or 1
export const scrollDirectionAtom = atom(-1);

// mouse scroll interval (unit: ms)
export const scrollIntervalAtom = atom(0);

// Where the captured screen lies in the target's desktop, as the board
// keeps it; null while the screen is the whole desktop
// (docs/design/screen-area-v1.md).
export const screenAreaAtom = atom<ScreenArea | null>(null);
