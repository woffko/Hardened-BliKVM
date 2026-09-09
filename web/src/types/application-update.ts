export type ApplicationSlot = {
  slotId: string;
  version: string;
  sequence: string;
  minimumSystemSequence: string;
};

export type ApplicationUploadReceipt = {
  slot: ApplicationSlot;
  disposition: 'published' | 'already_present';
  manifestSha256: string;
  payloadBytes: string;
};

export type ApplicationActivationReceipt = {
  phase: 'awaiting_health';
  receipt: { operationId: string; targetSlotId: string };
};

export type ApplicationActivationIntent = {
  target: ApplicationSlot;
  current: ApplicationSlot;
  expectedGeneration: string;
};

export type ApplicationRollbackReason =
  | 'interrupted_prepare'
  | 'boot_changed'
  | 'health_timeout'
  | 'explicit_health_failure'
  | 'pointer_mismatch'
  | 'target_invalid'
  | 'health_challenge_lost';

export type ApplicationUpdateStatus = {
  schemaVersion: 1;
  generation: string;
  floorSequence: string;
  systemSequence: string;
  selected: ApplicationSlot;
  confirmed: ApplicationSlot;
  previous: ApplicationSlot | null;
  pending: {
    operationId: string;
    target: ApplicationSlot;
    phase: 'prepared' | 'awaiting_health';
  } | null;
  lastRollback: ApplicationRollbackReason | null;
};
