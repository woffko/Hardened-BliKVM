import * as api from '@/api/restricted-control.ts';

import { newOperationId } from './hid-lease.ts';

const FOCUS_TTL_MS = 25_000;
const TARGET_HASH_BYTES = 16;

let confirmedAtMs = 0;

/**
 * Derives the opaque target identity the server expects from the stable
 * platform id. The server never trusts caller bindings directly; it only uses
 * this value to derive the attestation binding after an explicit confirmation.
 */
export async function targetIdFor(platformId: string): Promise<string> {
  const encoded = new TextEncoder().encode(`hardened-blikvm-target-v1:${platformId}`);
  const digest = await crypto.subtle.digest('SHA-256', encoded);
  return Array.from(new Uint8Array(digest).slice(0, TARGET_HASH_BYTES), (byte) =>
    byte.toString(16).padStart(2, '0')
  ).join('');
}

/** Ensures a recent safe-target/focus confirmation for the current platform. */
export async function ensureFocus(platformId: string): Promise<void> {
  if (Date.now() - confirmedAtMs < FOCUS_TTL_MS) {
    return;
  }
  const challenge = await api.getFocusChallenge();
  const confirmation = await api.confirmFocus(
    newOperationId(),
    challenge.data.challenge,
    await targetIdFor(platformId),
    true
  );
  if (!confirmation.data.attested) {
    throw new Error(confirmation.data.rejection ?? 'focus_not_attested');
  }
  confirmedAtMs = Date.now();
}

export async function performPowerAction(
  platformId: string,
  action: 'power_short' | 'power_long' | 'reset'
): Promise<api.PowerResult> {
  await ensureFocus(platformId);
  const response = await api.submitPowerAction(newOperationId(), action);
  return response.data;
}
