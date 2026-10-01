import { atom } from 'jotai';

import { getTransportChoice } from '@/lib/localstorage.ts';
import type { QuicStatus } from '@/lib/quic/session.ts';

// The transport this browser uses (Settings > Network > Transport).
export const transportChoiceAtom = atom<'standard' | 'quic'>(getTransportChoice());

// What the QUIC session is doing; `off` while QUIC is not chosen.
export const quicStatusAtom = atom<QuicStatus>({ state: 'off' });
