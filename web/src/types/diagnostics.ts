export type DiagnosticMetricState = 'available' | 'unavailable';

export type DiagnosticBooleanState = 'active' | 'inactive' | 'unavailable';

export type DiagnosticTemperature = {
  state: DiagnosticMetricState;
  millidegreesCelsius: number | null;
};

export type DiagnosticRtc = {
  state: DiagnosticMetricState;
  unixSeconds: number | null;
  offsetFromSystemSeconds: number | null;
};

export type DiagnosticComponentHealth = {
  enabled: DiagnosticBooleanState;
  online: DiagnosticBooleanState;
  busy: DiagnosticBooleanState;
  connected: DiagnosticBooleanState;
};

export type DiagnosticsView = {
  schemaVersion: 1;
  availability: 'current_only';
  current: {
    temperature: DiagnosticTemperature;
    rtc: DiagnosticRtc;
    components: {
      video: DiagnosticComponentHealth;
      hid: DiagnosticComponentHealth;
      power: DiagnosticComponentHealth;
      virtualMedia: DiagnosticComponentHealth;
      powerLed: DiagnosticBooleanState;
    };
  };
  history: {
    state: 'unavailable';
    reason: 'persistence_not_provisioned';
  };
};
