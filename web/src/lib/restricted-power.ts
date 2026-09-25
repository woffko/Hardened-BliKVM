import * as api from '@/api/restricted-control.ts';

import { newOperationId } from './hid-lease.ts';

// The server attests for 30 s, so the client refreshes well inside that.
const FOCUS_TTL_MS = 15_000;
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
// The server keeps one outstanding challenge per session, so concurrent
// refreshes (a lease renewal and a queued report) must share one round trip:
// a second challenge supersedes the first and its confirmation is refused.
let pendingFocus: Promise<void> | null = null;

export function ensureFocus(platformId: string): Promise<void> {
  if (Date.now() - confirmedAtMs < FOCUS_TTL_MS) {
    return Promise.resolve();
  }
  if (!pendingFocus) {
    pendingFocus = confirmFocusOnce(platformId).finally(() => {
      pendingFocus = null;
    });
  }
  return pendingFocus;
}

async function confirmFocusOnce(platformId: string): Promise<void> {
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

/**
 * Drops the cached confirmation after a refusal, so the next attempt
 * re-confirms immediately instead of waiting for the cache to lapse.
 */
export function resetFocusConfirmation(): void {
  confirmedAtMs = 0;
}

export async function performPowerAction(
  platformId: string,
  action: 'power_short' | 'power_long' | 'reset'
): Promise<api.PowerResult> {
  await ensureFocus(platformId);
  const response = await api.submitPowerAction(newOperationId(), action);
  return response.data;
}
