export type NetworkSectionStatusKind = 'error' | 'success' | 'warning' | '';

export type NetworkSectionStatus = {
  hasPending: boolean;
  hasInvalid: boolean;
  canApply: boolean;
  isLoading: boolean;
  isSaving: boolean;
  statusText: string;
  statusKind: NetworkSectionStatusKind;
};

export type NetworkSectionResult = {
  changed: boolean;
  error?: boolean;
  redirecting?: boolean;
};

export type NetworkSectionHandle = {
  apply: () => Promise<NetworkSectionResult>;
};

export const idleNetworkSectionStatus: NetworkSectionStatus = {
  hasPending: false,
  hasInvalid: false,
  canApply: false,
  isLoading: false,
  isSaving: false,
  statusText: '',
  statusKind: ''
};
