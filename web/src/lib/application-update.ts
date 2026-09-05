import type { ApplicationSlot, ApplicationUpdateStatus } from '@/types/application-update.ts';

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
