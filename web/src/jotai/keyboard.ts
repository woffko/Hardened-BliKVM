import { atom } from 'jotai';

// is the keyboard enabled (Disable keyboard events when input is required)
export const isKeyboardEnableAtom = atom(true);

// is the virtual keyboard opened
export const isKeyboardOpenAtom = atom(false);

// is the mobile browser soft keyboard bridge opened
export const isLocalKeyboardOpenAtom = atom(false);

// Heights in px that the touch keyboards cover at the bottom of the page: the
// on-screen keyboard, and the key helper together with the system keyboard
// under it. The screen is laid out above the larger one.
export const virtualKeyboardHeightAtom = atom(0);
export const keyHelperHeightAtom = atom(0);

// leader key code for bypassing browser shortcuts (empty string means disabled)
export const leaderKeyAtom = atom('');
