import type { CapabilityManifest } from '@/types/capabilities.ts';
import { atom } from 'jotai';

export const capabilitiesAtom = atom<CapabilityManifest | null>(null);
export const capabilitiesLoadingAtom = atom(true);
export const capabilitiesErrorAtom = atom<string | null>(null);
