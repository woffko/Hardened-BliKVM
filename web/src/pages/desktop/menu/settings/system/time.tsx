import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import { Alert, Button, Divider, Input, message, Select, Switch } from 'antd';
import { ClockIcon, PlusIcon, RefreshCwIcon, RouterIcon, Trash2Icon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/system-time.ts';
import type { TimeConfig, TimeConfigResponse } from '@/api/system-time.ts';

import type { SystemActionHandle, SystemActionState } from './action.ts';

const defaultConfig: TimeConfig = {
  ntpEnabled: true,
  timezone: 'Etc/UTC',
  servers: ['0.pool.ntp.org', '1.pool.ntp.org', '2.pool.ntp.org', '3.pool.ntp.org']
};

type TimeSettingsProps = {
  showFooter?: boolean;
  onActionStateChange?: (state: SystemActionState) => void;
};

export const TimeSettings = forwardRef<SystemActionHandle, TimeSettingsProps>(
  ({ showFooter = true, onActionStateChange }, ref) => {
    const { t } = useTranslation();

    const [data, setData] = useState<TimeConfigResponse | null>(null);
    const [config, setConfig] = useState<TimeConfig>(defaultConfig);
    const [savedConfig, setSavedConfig] = useState<TimeConfig>(defaultConfig);
    const [isLoading, setIsLoading] = useState(false);
    const [isSaving, setIsSaving] = useState(false);
    const [isSyncing, setIsSyncing] = useState(false);

    const timezoneOptions = useMemo(
      () =>
        (data?.timezoneOptions || [config.timezone]).map((zone) => ({
          value: zone,
          label: zone
        })),
      [data?.timezoneOptions, config.timezone]
    );

    function patchConfig(patch: Partial<TimeConfig>) {
      setConfig((current) => ({ ...current, ...patch }));
    }

    const load = useCallback(async () => {
      setIsLoading(true);
      try {
        const rsp = await api.getConfig();
        if (rsp.code !== 0 || !rsp.data) {
          message.error(rsp.msg || t('settings.system.time.loadFailed'));
          return;
        }

        const nextConfig = { ...defaultConfig, ...rsp.data.config };
        setData(rsp.data);
        setConfig(nextConfig);
        setSavedConfig(nextConfig);
      } catch {
        message.error(t('settings.system.time.loadFailed'));
      } finally {
        setIsLoading(false);
      }
    }, [t]);

    useEffect(() => {
      void load();
    }, [load]);

    function changeNtpEnabled(ntpEnabled: boolean) {
      if (ntpEnabled && config.servers.length === 0) {
        patchConfig({
          ntpEnabled,
          servers: data?.gateway ? [data.gateway] : data?.defaultServers || defaultConfig.servers
        });
        return;
      }

      patchConfig({ ntpEnabled });
    }

    function updateServer(index: number, value: string) {
      setConfig((current) => ({
        ...current,
        servers: current.servers.map((server, itemIndex) => (itemIndex === index ? value : server))
      }));
    }

    function removeServer(index: number) {
      setConfig((current) => ({
        ...current,
        servers: current.servers.filter((_, itemIndex) => itemIndex !== index)
      }));
    }

    function addServer() {
      if (config.servers.length >= 6) {
        message.error(t('settings.system.time.maxServers', { count: 6 }));
        return;
      }
      patchConfig({ servers: [...config.servers, ''] });
    }

    function useRouter() {
      if (!data?.gateway) return;
      patchConfig({ servers: [data.gateway] });
    }

    function useDefaults() {
      patchConfig({ servers: data?.defaultServers || defaultConfig.servers });
    }

    const save = useCallback(async () => {
      if (isSaving) return;

      const servers = config.servers.map((server) => server.trim()).filter(Boolean);
      if (config.ntpEnabled && servers.length === 0) {
        message.error(t('settings.system.time.invalidServers'));
        return;
      }

      setIsSaving(true);
      try {
        const rsp = await api.setConfig({ ...config, servers });
        if (rsp.code !== 0 || !rsp.data) {
          message.error(rsp.msg || t('settings.system.time.saveFailed'));
          return;
        }

        const nextConfig = { ...defaultConfig, ...rsp.data.config };
        setData(rsp.data);
        setConfig(nextConfig);
        setSavedConfig(nextConfig);
        message.success(t('settings.system.time.saved'));
      } catch (err) {
        console.log(err);
        message.error(t('settings.system.time.saveFailed'));
      } finally {
        setIsSaving(false);
      }
    }, [config, isSaving, t]);

    async function syncNow() {
      if (isSyncing) return;

      setIsSyncing(true);
      try {
        const rsp = await api.syncNow();
        if (rsp.code !== 0 || !rsp.data) {
          message.error(rsp.msg || t('settings.system.time.syncFailed'));
          return;
        }

        const nextConfig = { ...defaultConfig, ...rsp.data.config };
        setData(rsp.data);
        setConfig(nextConfig);
        setSavedConfig(nextConfig);
        message.success(t('settings.system.time.synced'));
      } catch (err) {
        console.log(err);
        message.error(t('settings.system.time.syncFailed'));
      } finally {
        setIsSyncing(false);
      }
    }

    const normalizedConfig = normalizeTimeConfig(config);
    const normalizedSavedConfig = normalizeTimeConfig(savedConfig);
    const hasPending = JSON.stringify(normalizedConfig) !== JSON.stringify(normalizedSavedConfig);
    const hasInvalid = normalizedConfig.ntpEnabled && normalizedConfig.servers.length === 0;
    const isBusy = isLoading || isSaving || isSyncing;

    useImperativeHandle(ref, () => ({ save }), [save]);

    useEffect(() => {
      onActionStateChange?.({
        hasPending,
        hasInvalid,
        canSave: hasPending && !hasInvalid && !isBusy,
        isBusy,
        statusText: hasInvalid
          ? t('settings.system.time.invalidServers')
          : hasPending
            ? t('settings.system.unsaved')
            : '',
        statusKind: hasInvalid ? 'error' : hasPending ? 'warning' : '',
        label: t('settings.system.time.save')
      });
    }, [hasInvalid, hasPending, isBusy, onActionStateChange, t]);

    return (
      <div className="flex min-w-0 flex-col space-y-6">
        <div className="flex min-w-0 items-start justify-between gap-3 sm:gap-4">
          <div className="flex min-w-0 flex-col space-y-1">
            <span>{t('settings.system.time.ntp.title')}</span>
            <span className="break-words text-xs text-neutral-500">
              {t('settings.system.time.ntp.description')}
            </span>
          </div>
          <Switch
            className="shrink-0"
            checked={config.ntpEnabled}
            loading={isLoading}
            onChange={changeNtpEnabled}
          />
        </div>

        <div className="min-w-0 space-y-3 overflow-hidden rounded-lg bg-neutral-800/50 p-4">
          <div>
            <div className="font-semibold text-neutral-100">
              {t('settings.system.time.clock.title')}
            </div>
            <div className="mt-0.5 break-words text-xs leading-snug text-neutral-500">
              {t('settings.system.time.clock.description')}
            </div>
          </div>

          <SettingRow label={t('settings.system.time.clock.current')}>
            <div className="flex min-h-[32px] items-center gap-2 text-sm text-neutral-300">
              <ClockIcon size={15} />
              <span>{data?.currentTime || '-'}</span>
            </div>
          </SettingRow>

          <SettingRow label={t('settings.system.time.clock.timezone')} isLast>
            <Select
              showSearch
              value={config.timezone}
              options={timezoneOptions}
              onChange={(timezone) => patchConfig({ timezone })}
              filterOption={(input, option) =>
                String(option?.label || '')
                  .toLowerCase()
                  .includes(input.toLowerCase())
              }
              className="w-full"
            />
          </SettingRow>
        </div>

        {config.ntpEnabled && (
          <div className="min-w-0 space-y-3 overflow-hidden rounded-lg bg-neutral-800/50 p-4">
            <div>
              <div className="font-semibold text-neutral-100">
                {t('settings.system.time.servers.title')}
              </div>
              <div className="mt-0.5 break-words text-xs leading-snug text-neutral-500">
                {t('settings.system.time.servers.description')}
              </div>
            </div>

            <div className="space-y-2">
              {config.servers.map((server, index) => (
                <div key={index} className="flex min-w-0 gap-2">
                  <Input
                    className="min-w-0"
                    value={server}
                    placeholder="0.pool.ntp.org"
                    onChange={(event) => updateServer(index, event.target.value)}
                  />
                  <Button
                    icon={<Trash2Icon size={15} />}
                    disabled={config.servers.length <= 1}
                    onClick={() => removeServer(index)}
                  />
                </div>
              ))}
            </div>

            <div className="flex min-w-0 flex-wrap justify-end gap-2">
              <Button className="min-w-0" icon={<PlusIcon size={15} />} onClick={addServer}>
                {t('settings.system.time.servers.add')}
              </Button>
              <Button
                className="min-w-0"
                disabled={!data?.gateway}
                icon={<RouterIcon size={15} />}
                onClick={useRouter}
              >
                {t('settings.system.time.servers.useRouter')}
              </Button>
              <Button className="min-w-0" onClick={useDefaults}>
                {t('settings.system.time.servers.useDefaults')}
              </Button>
            </div>

            {data?.gateway && (
              <Alert
                type="info"
                showIcon
                message={t('settings.system.time.servers.routerDetected', {
                  gateway: data.gateway
                })}
              />
            )}
          </div>
        )}

        <Divider className="opacity-50" />

        <div className="flex min-w-0 flex-wrap justify-end gap-2">
          <Button
            className="min-w-0"
            icon={<RefreshCwIcon size={16} />}
            loading={isLoading}
            onClick={load}
          >
            {t('settings.system.time.refresh')}
          </Button>
          <Button
            className="min-w-0"
            icon={<ClockIcon size={16} />}
            disabled={!config.ntpEnabled}
            loading={isSyncing}
            onClick={syncNow}
          >
            {t('settings.system.time.syncNow')}
          </Button>
          {showFooter && (
            <Button
              type="primary"
              loading={isSaving}
              disabled={!hasPending || hasInvalid}
              onClick={() => void save()}
            >
              {t('settings.system.time.save')}
            </Button>
          )}
        </div>
      </div>
    );
  }
);

TimeSettings.displayName = 'TimeSettings';

function normalizeTimeConfig(config: TimeConfig): TimeConfig {
  return {
    ntpEnabled: Boolean(config.ntpEnabled),
    timezone: config.timezone || defaultConfig.timezone,
    servers: config.servers.map((server) => server.trim()).filter(Boolean)
  };
}

const SettingRow = ({
  label,
  children,
  isLast = false
}: {
  label: string;
  children: ReactNode;
  isLast?: boolean;
}) => {
  return (
    <div
      className={`flex min-h-[44px] min-w-0 flex-col items-stretch gap-2 sm:flex-row sm:items-center sm:justify-between sm:gap-4 ${
        isLast ? '' : 'border-b border-neutral-700/50 pb-3'
      }`}
    >
      <span className="min-w-0 text-sm text-neutral-300">{label}</span>
      <div className="w-full min-w-0 sm:w-[280px]">{children}</div>
    </div>
  );
};
