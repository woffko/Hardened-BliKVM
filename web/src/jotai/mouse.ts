import { atom } from 'jotai';

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
