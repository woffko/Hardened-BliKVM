import { atom } from 'jotai';

/**
 * Set only after the operator explicitly confirms that the KVM video shows the
 * intended target. Restricted HID/power actions must not proceed without it.
 * The confirmation holds for the page's life: the lease it allows is given
 * back when the window loses focus and taken again by the next input.
 */
export const focusConfirmedAtom = atom(false);

/**
 * Counts the leases given back. The target holds nothing pressed once its
 * lease ends, so what the page shows as held (a modifier of the extra keys
 * row) is let go too.
 */
export const leaseEndedAtom = atom(0);
