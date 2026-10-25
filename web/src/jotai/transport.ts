import { atom } from 'jotai';

import { getTransportChoice } from '@/lib/localstorage.ts';
import type { QuicStatus, QuicVideoStatus } from '@/lib/quic/session.ts';
import type { VideoStats } from '@/lib/quic/video.worker.ts';

// The transport this browser uses (Settings > Network > Transport).
export const transportChoiceAtom = atom<'standard' | 'quic'>(getTransportChoice());

// What the QUIC session is doing; `off` while QUIC is not chosen.
export const quicStatusAtom = atom<QuicStatus>({ state: 'off' });

// What carries the keyboard and the mouse while remote control is on; `null`
// while it is off. It is chosen when control is enabled.
export const leaseTransportAtom = atom<'quic' | 'https' | null>(null);

// The picture over the QUIC session: whether the page watches it, and what
// the gateway said about it.
export const quicVideoAtom = atom<QuicVideoStatus>({ state: 'off' });

// What the decoder has drawn lately, while the picture comes over QUIC.
export const quicVideoStatsAtom = atom<VideoStats | null>(null);

// The picture over QUIC was tried and is not to be had on this page load:
// the reason, shown where the page says what it uses instead.
export const quicVideoGivenUpAtom = atom<string | null>(null);
