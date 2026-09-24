import type {
  ApplicationActivationIntent,
  ApplicationActivationReceipt,
  ApplicationSlot,
  ApplicationUpdateStatus,
  ApplicationUploadReceipt
} from '@/types/application-update.ts';

export const MAX_APPLICATION_ARCHIVE_BYTES = 128 * 1024 * 1024;

export function validApplicationArchiveSize(size: number) {
  return Number.isSafeInteger(size) && size > 0 && size <= MAX_APPLICATION_ARCHIVE_BYTES;
}

const MAX_COUNTER = '18446744073709551615';
const ROLLBACK_REASONS = new Set([
  'interrupted_prepare',
  'boot_changed',
  'health_timeout',
  'explicit_health_failure',
  'pointer_mismatch',
  'target_invalid',
  'health_challenge_lost'
]);

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function keys(value: Record<string, unknown>, names: string[]) {
  return (
    Object.keys(value).length === names.length &&
    names.every((name) => Object.prototype.hasOwnProperty.call(value, name))
  );
}

function counter(value: unknown): value is string {
  return (
    typeof value === 'string' &&
    /^[1-9][0-9]{0,19}$/.test(value) &&
    (value.length < MAX_COUNTER.length || value <= MAX_COUNTER)
  );
}

function atMost(left: string, right: string) {
  return left.length < right.length || (left.length === right.length && left <= right);
}

function slot(value: unknown): value is ApplicationSlot {
  if (
    !object(value) ||
    !keys(value, ['slotId', 'version', 'sequence', 'minimumSystemSequence']) ||
    !counter(value.sequence) ||
    !counter(value.minimumSystemSequence) ||
    typeof value.slotId !== 'string' ||
    typeof value.version !== 'string' ||
    !/^[a-zA-Z0-9.+-]{1,64}$/.test(value.version)
  )
    return false;
  const match = /^app-([0-9]{20})-([a-f0-9]{64})$/.exec(value.slotId);
  return !!match && match[1] === value.sequence.padStart(20, '0') && !/^0+$/.test(match[2]);
}

function sameSlot(left: ApplicationSlot, right: ApplicationSlot) {
  return (
    left.slotId === right.slotId &&
    left.version === right.version &&
    left.sequence === right.sequence &&
    left.minimumSystemSequence === right.minimumSystemSequence
  );
}

export function eligibleApplicationTarget(
  target: ApplicationSlot,
  status: ApplicationUpdateStatus
) {
  return (
    status.pending === null &&
    !atMost(target.sequence, status.floorSequence) &&
    atMost(target.minimumSystemSequence, status.systemSequence) &&
    target.slotId !== status.selected.slotId
  );
}

export function currentApplicationIntent(
  intent: ApplicationActivationIntent,
  status: ApplicationUpdateStatus
) {
  return (
    intent.expectedGeneration === status.generation &&
    sameSlot(intent.current, status.confirmed) &&
    eligibleApplicationTarget(intent.target, status)
  );
}

export function isApplicationActivationReceipt(
  value: unknown,
  target: string
): value is ApplicationActivationReceipt {
  if (
    !object(value) ||
    !keys(value, ['phase', 'receipt']) ||
    (value.phase !== 'prepared' && value.phase !== 'awaiting_health')
  )
    return false;
  const receipt = value.receipt;
  return (
    object(receipt) &&
    keys(receipt, ['operationId', 'targetSlotId']) &&
    receipt.targetSlotId === target &&
    typeof receipt.operationId === 'string' &&
    /^[a-f0-9]{32}$/.test(receipt.operationId) &&
    !/^0+$/.test(receipt.operationId)
  );
}

export function reconcileApplicationActivation(
  intent: ApplicationActivationIntent,
  receipt: ApplicationActivationReceipt | null,
  status: ApplicationUpdateStatus
): 'awaiting' | 'uncertain' | 'installed' | 'notApplied' | 'changed' {
  // Old/in-flight reads do not settle a mutation, even if HTTP has replied.
  if (atMost(status.generation, intent.expectedGeneration))
    return receipt ? 'awaiting' : 'uncertain';
  if (status.pending) {
    return sameSlot(status.pending.target, intent.target) &&
      (!receipt || receipt.receipt.operationId === status.pending.operationId)
      ? 'awaiting'
      : 'changed';
  }
  // This proves current confirmed target, not attribution to a particular request.
  return sameSlot(status.confirmed, intent.target) ? 'installed' : 'notApplied';
}

export function isApplicationUploadReceipt(value: unknown): value is ApplicationUploadReceipt {
  return (
    object(value) &&
    keys(value, ['slot', 'disposition', 'manifestSha256', 'payloadBytes']) &&
    slot(value.slot) &&
    (value.disposition === 'published' || value.disposition === 'already_present') &&
    value.manifestSha256 === value.slot.slotId.slice(25) &&
    counter(value.payloadBytes) &&
    atMost(value.payloadBytes, String(MAX_APPLICATION_ARCHIVE_BYTES))
  );
}

export function isApplicationUpdateStatus(value: unknown): value is ApplicationUpdateStatus {
  if (
    !object(value) ||
    !keys(value, [
      'schemaVersion',
      'generation',
      'floorSequence',
      'systemSequence',
      'selected',
      'confirmed',
      'previous',
      'pending',
      'lastRollback'
    ]) ||
    value.schemaVersion !== 1 ||
    !counter(value.generation) ||
    !counter(value.floorSequence) ||
    !counter(value.systemSequence) ||
    !slot(value.selected) ||
    !slot(value.confirmed) ||
    value.floorSequence !== value.confirmed.sequence ||
    !atMost(value.selected.minimumSystemSequence, value.systemSequence) ||
    !atMost(value.confirmed.minimumSystemSequence, value.systemSequence) ||
    (value.previous !== null && !slot(value.previous)) ||
    (value.lastRollback !== null &&
      (typeof value.lastRollback !== 'string' || !ROLLBACK_REASONS.has(value.lastRollback)))
  )
    return false;
  if (value.pending === null) return sameSlot(value.selected, value.confirmed);
  const pending = value.pending;
  if (
    !object(pending) ||
    !keys(pending, ['operationId', 'target', 'phase']) ||
    typeof pending.operationId !== 'string' ||
    !/^[a-f0-9]{32}$/.test(pending.operationId) ||
    /^0+$/.test(pending.operationId) ||
    !slot(pending.target) ||
    atMost(pending.target.sequence, value.floorSequence) ||
    !atMost(pending.target.minimumSystemSequence, value.systemSequence) ||
    (pending.phase !== 'prepared' && pending.phase !== 'awaiting_health')
  )
    return false;
  return pending.phase === 'awaiting_health'
    ? sameSlot(value.selected, pending.target)
    : sameSlot(value.selected, value.confirmed) || sameSlot(value.selected, pending.target);
}
