import { atom } from 'jotai';

import { getLayoutMode, getWebTitle, type LayoutMode } from '@/lib/localstorage.ts';

// menu bar disabled items
export const menuDisabledItemsAtom = atom<string[]>([]);

// track how many submenus are currently open
export const submenuOpenCountAtom = atom(0);

// web title
export const webTitleAtom = atom(getWebTitle());

// menu display mode: 'off' | 'auto' | 'always'
export const menuDisplayModeAtom = atom<string>('auto');

// On the touch layout, whether the menu bar is showing over the top of the
// page; the screen is then laid out below it instead of under it.
export const menuBarOccupiesTopAtom = atom(false);

// responsive layout mode: 'auto' | 'mobile'
export const layoutModeAtom = atom<LayoutMode>(getLayoutMode());

// selected settings tab, so sections can link to each other
export const settingsTabAtom = atom('about');
