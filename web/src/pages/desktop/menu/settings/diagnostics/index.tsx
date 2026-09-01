import type { ReactNode } from 'react';
import type { DiagnosticBooleanState } from '@/types/diagnostics.ts';
import { Alert, Button, Divider, Spin } from 'antd';
import clsx from 'clsx';
import {
  ActivityIcon,
  Clock3Icon,
  HardDriveIcon,
  HistoryIcon,
  KeyboardIcon,
  LightbulbIcon,
  MonitorIcon,
  RefreshCwIcon,
  ServerIcon,
  ThermometerIcon
} from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { formatDiagnosticsUpdatedTime, primaryComponentState } from '@/lib/diagnostics.ts';
import { formatRtcOffsetSeconds, formatRtcUnixSeconds } from '@/lib/rtc.ts';
import { formatTemperatureMillidegrees } from '@/lib/thermal.ts';
import { useDiagnostics } from '@/hooks/useDiagnostics.ts';

type CardProps = {
  icon: ReactNode;
  label: string;
  value: ReactNode;
};

const DiagnosticCard = ({ icon, label, value }: CardProps) => (
  <div className="min-w-0 rounded-lg border border-neutral-700/70 bg-neutral-800/40 p-3">
    <div className="flex items-center gap-2 text-xs text-neutral-400">
      <span className="text-neutral-500">{icon}</span>
      <span className="truncate">{label}</span>
    </div>
    <div className="mt-2 min-w-0 break-words text-sm text-neutral-100">{value}</div>
  </div>
);

const StateValue = ({ state }: { state: DiagnosticBooleanState }) => {
  const { t } = useTranslation();
  return (
    <span
      className={clsx(
        'inline-flex rounded-full px-2 py-0.5 text-xs font-medium',
        state === 'active' && 'bg-emerald-500/15 text-emerald-300',
        state === 'inactive' && 'bg-neutral-700 text-neutral-300',
        state === 'unavailable' && 'bg-amber-500/15 text-amber-300'
      )}
    >
      {t(`settings.diagnostics.state.${state}`)}
    </span>
  );
};

export const Diagnostics = () => {
  const { t, i18n } = useTranslation();
  const { data, state, lastUpdated, refreshing, refresh } = useDiagnostics();
  const updated = lastUpdated ? formatDiagnosticsUpdatedTime(lastUpdated, i18n.language) : null;

  return (
    <>
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="text-base">{t('settings.diagnostics.title')}</div>
          <div className="mt-1 text-sm text-neutral-500">
            {t('settings.diagnostics.description')}
          </div>
        </div>
        <Button
          size="small"
          icon={<RefreshCwIcon size={14} />}
          loading={refreshing}
          disabled={refreshing}
          onClick={() => void refresh()}
        >
          {t('settings.diagnostics.refresh')}
        </Button>
      </div>
      <Divider className="opacity-50" />

      {state === 'loading' ? (
        <div className="flex min-h-40 items-center justify-center gap-3 text-neutral-400">
          <Spin size="small" />
          <span>{t('settings.diagnostics.loading')}</span>
        </div>
      ) : null}

      {state === 'error' ? (
        <Alert type="error" showIcon message={t('settings.diagnostics.loadFailed')} />
      ) : null}

      {state === 'stale' ? (
        <Alert type="warning" showIcon message={t('settings.diagnostics.stale')} />
      ) : null}

      {data ? (
        <div className="space-y-6">
          <div className="rounded-lg border border-blue-500/20 bg-blue-500/10 p-3 text-sm text-blue-100">
            <div>{t('settings.diagnostics.currentOnly')}</div>
            {updated ? (
              <div className="mt-1 text-xs text-blue-200/70">
                {t('settings.diagnostics.lastUpdated', { time: updated })}
              </div>
            ) : null}
          </div>

          <section>
            <div className="mb-3 flex items-center gap-2 text-sm text-neutral-400">
              <ActivityIcon size={15} />
              {t('settings.diagnostics.sensors')}
            </div>
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <DiagnosticCard
                icon={<ThermometerIcon size={15} />}
                label={t('settings.diagnostics.temperature')}
                value={
                  data.current.temperature.state === 'available'
                    ? formatTemperatureMillidegrees(data.current.temperature.millidegreesCelsius)
                    : t('settings.diagnostics.state.unavailable')
                }
              />
              <DiagnosticCard
                icon={<Clock3Icon size={15} />}
                label={t('settings.diagnostics.externalRtc')}
                value={
                  data.current.rtc.state === 'available'
                    ? formatRtcUnixSeconds(data.current.rtc.unixSeconds)
                    : t('settings.diagnostics.state.unavailable')
                }
              />
              <DiagnosticCard
                icon={<Clock3Icon size={15} />}
                label={t('settings.diagnostics.rtcOffset')}
                value={
                  data.current.rtc.state === 'available'
                    ? formatRtcOffsetSeconds(data.current.rtc.offsetFromSystemSeconds)
                    : t('settings.diagnostics.state.unavailable')
                }
              />
            </div>
          </section>

          <section>
            <div className="mb-3 flex items-center gap-2 text-sm text-neutral-400">
              <ServerIcon size={15} />
              {t('settings.diagnostics.components')}
            </div>
            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <DiagnosticCard
                icon={<MonitorIcon size={15} />}
                label={t('settings.diagnostics.video')}
                value={<StateValue state={primaryComponentState(data.current.components.video)} />}
              />
              <DiagnosticCard
                icon={<KeyboardIcon size={15} />}
                label={t('settings.diagnostics.hid')}
                value={<StateValue state={primaryComponentState(data.current.components.hid)} />}
              />
              <DiagnosticCard
                icon={<ActivityIcon size={15} />}
                label={t('settings.diagnostics.power')}
                value={<StateValue state={primaryComponentState(data.current.components.power)} />}
              />
              <DiagnosticCard
                icon={<HardDriveIcon size={15} />}
                label={t('settings.diagnostics.virtualMedia')}
                value={
                  <StateValue
                    state={primaryComponentState(data.current.components.virtualMedia, true)}
                  />
                }
              />
              <DiagnosticCard
                icon={<LightbulbIcon size={15} />}
                label={t('settings.diagnostics.powerLed')}
                value={<StateValue state={data.current.components.powerLed} />}
              />
            </div>
          </section>

          <section>
            <div className="mb-3 flex items-center gap-2 text-sm text-neutral-400">
              <HistoryIcon size={15} />
              {t('settings.diagnostics.history')}
            </div>
            <Alert type="info" showIcon message={t('settings.diagnostics.historyNotProvisioned')} />
          </section>
        </div>
      ) : null}
    </>
  );
};
