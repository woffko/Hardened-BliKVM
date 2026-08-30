export type SystemActionStatusKind = 'error' | 'success' | 'warning' | '';

export type SystemActionState = {
  hasPending: boolean;
  hasInvalid: boolean;
  canSave: boolean;
  isBusy: boolean;
  statusText: string;
  statusKind: SystemActionStatusKind;
  label: string;
};

export type SystemActionHandle = {
  save: () => void | Promise<void>;
};

export const idleSystemActionState: SystemActionState = {
  hasPending: false,
  hasInvalid: false,
  canSave: false,
  isBusy: false,
  statusText: '',
  statusKind: '',
  label: ''
};
