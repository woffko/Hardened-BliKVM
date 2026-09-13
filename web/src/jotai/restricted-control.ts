import { atom } from 'jotai';

/**
 * Set only after the operator explicitly confirms that the KVM video shows the
 * intended target. Restricted HID/power actions must not proceed without it.
 */
export const focusConfirmedAtom = atom(false);
