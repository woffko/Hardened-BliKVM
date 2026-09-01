import type {
  DiagnosticBooleanState,
  DiagnosticComponentHealth,
  DiagnosticMetricState,
  DiagnosticsView
} from '@/types/diagnostics.ts';

const BOOLEAN_STATES = new Set<DiagnosticBooleanState>(['active', 'inactive', 'unavailable']);
const METRIC_STATES = new Set<DiagnosticMetricState>(['available', 'unavailable']);
const INTL_LOCALE_ALIASES: Record<string, string> = {
  cz: 'cs',
  no: 'nb',
  pt_br: 'pt-BR',
  se: 'sv',
  zh_tw: 'zh-TW'
};

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function hasExactKeys(value: Record<string, unknown>, expected: readonly string[]) {
  const actual = Object.keys(value).sort();
  const required = [...expected].sort();
  return actual.length === required.length && actual.every((key, index) => key === required[index]);
}

function isBooleanState(value: unknown): value is DiagnosticBooleanState {
  return typeof value === 'string' && BOOLEAN_STATES.has(value as DiagnosticBooleanState);
}

function isMetricState(value: unknown): value is DiagnosticMetricState {
  return typeof value === 'string' && METRIC_STATES.has(value as DiagnosticMetricState);
}

function isIntegerInRange(value: unknown, minimum: number, maximum: number) {
  return (
    typeof value === 'number' && Number.isInteger(value) && value >= minimum && value <= maximum
  );
}

function isComponent(value: unknown): value is DiagnosticComponentHealth {
  return (
    isObject(value) &&
    hasExactKeys(value, ['enabled', 'online', 'busy', 'connected']) &&
    isBooleanState(value.enabled) &&
    isBooleanState(value.online) &&
    isBooleanState(value.busy) &&
    isBooleanState(value.connected)
  );
}

export function isDiagnosticsView(value: unknown): value is DiagnosticsView {
  if (
    !isObject(value) ||
    !hasExactKeys(value, ['schemaVersion', 'availability', 'current', 'history']) ||
    value.schemaVersion !== 1 ||
    value.availability !== 'current_only'
  ) {
    return false;
  }
  if (!isObject(value.current) || !isObject(value.history)) return false;
  if (
    !hasExactKeys(value.current, ['temperature', 'rtc', 'components']) ||
    !hasExactKeys(value.history, ['state', 'reason'])
  ) {
    return false;
  }
  if (
    value.history.state !== 'unavailable' ||
    value.history.reason !== 'persistence_not_provisioned'
  ) {
    return false;
  }

  const { temperature, rtc, components } = value.current;
  if (
    !isObject(temperature) ||
    !hasExactKeys(temperature, ['state', 'millidegreesCelsius']) ||
    !isMetricState(temperature.state)
  ) {
    return false;
  }
  const validTemperature =
    (temperature.state === 'unavailable' && temperature.millidegreesCelsius === null) ||
    (temperature.state === 'available' &&
      isIntegerInRange(temperature.millidegreesCelsius, -40_000, 150_000));
  if (!validTemperature) return false;

  if (
    !isObject(rtc) ||
    !hasExactKeys(rtc, ['state', 'unixSeconds', 'offsetFromSystemSeconds']) ||
    !isMetricState(rtc.state)
  ) {
    return false;
  }
  const validRtc =
    (rtc.state === 'unavailable' &&
      rtc.unixSeconds === null &&
      rtc.offsetFromSystemSeconds === null) ||
    (rtc.state === 'available' &&
      isIntegerInRange(rtc.unixSeconds, 946_684_800, 4_102_444_799) &&
      isIntegerInRange(rtc.offsetFromSystemSeconds, -315_576_000, 315_576_000));
  if (
    !validRtc ||
    !isObject(components) ||
    !hasExactKeys(components, ['video', 'hid', 'power', 'virtualMedia', 'powerLed'])
  ) {
    return false;
  }

  return (
    isComponent(components.video) &&
    isComponent(components.hid) &&
    isComponent(components.power) &&
    isComponent(components.virtualMedia) &&
    isBooleanState(components.powerLed)
  );
}

export function primaryComponentState(
  component: DiagnosticComponentHealth,
  preferConnected = false
): DiagnosticBooleanState {
  if (preferConnected && component.connected !== 'unavailable') return component.connected;
  if (component.online !== 'unavailable') return component.online;
  return component.enabled;
}

export function formatDiagnosticsUpdatedTime(timestamp: number, language: string) {
  const localeKey = language.trim().toLowerCase();
  const locale = INTL_LOCALE_ALIASES[localeKey] || localeKey.replace('_', '-');

  try {
    return new Intl.DateTimeFormat(locale, {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit'
    }).format(timestamp);
  } catch {
    return new Date(timestamp).toISOString().slice(11, 19) + ' UTC';
  }
}
