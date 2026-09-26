import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import type { RuntimeInventory } from '@/types/inventory.ts';
import { useAtomValue } from 'jotai';
import { useTranslation } from 'react-i18next';

import { getApplicationUpdateStatus } from '@/api/application-update.ts';
import { getCurrentVersion } from '@/api/application.ts';
import { getRuntimeInventory } from '@/api/inventory.ts';
import { getInfo } from '@/api/vm.ts';
import { formatHardenedVersion } from '@/lib/hardened.ts';
import { formatRtcOffsetSeconds, formatRtcUnixSeconds } from '@/lib/rtc.ts';
import { formatTemperatureMillidegrees } from '@/lib/thermal.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';

type InfoRowProps = {
  label: ReactNode;
  value: ReactNode;
};

const InfoRow = ({ label, value }: InfoRowProps) => (
  <div className="flex w-full min-w-0 flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
    <div className="min-w-0 shrink-0 text-neutral-300 sm:max-w-[45%]">{label}</div>
    <div className="min-w-0 break-words text-left text-neutral-300 sm:text-right">{value}</div>
  </div>
);

export const Information = () => {
  const { t } = useTranslation();
  const capabilities = useAtomValue(capabilitiesAtom);
  const [inventory, setInventory] = useState<RuntimeInventory | null>(null);
  const [applicationVersion, setApplicationVersion] = useState('');
  const [runningSlot, setRunningSlot] = useState('');
  const [mdns, setMdns] = useState('');
  const [uptime, setUptime] = useState<number | null>(null);

  useEffect(() => {
    let cancelled = false;
    getRuntimeInventory()
      .then((response) => {
        if (!cancelled && response.code === 0 && response.data.schemaVersion === 3) {
          setInventory(response.data);
        }
      })
      .catch(() => undefined);
    // The signed slot that is running identifies the application precisely;
    // the crate version stays the same across development builds.
    getApplicationUpdateStatus()
      .then((response) => {
        const confirmed = response.data?.confirmed;
        if (!cancelled && response.code === 0 && confirmed?.version) {
          setRunningSlot(`${confirmed.version} (#${confirmed.sequence})`);
        }
      })
      .catch(() => undefined);
    getInfo()
      .then((response) => {
        if (cancelled || response.code !== 0) return;
        if (typeof response.data?.mdns === 'string') setMdns(response.data.mdns);
        if (Number.isInteger(response.data?.uptime)) setUptime(response.data.uptime);
      })
      .catch(() => undefined);
    getCurrentVersion()
      .then((response) => {
        if (!cancelled && response.code === 0 && typeof response.data?.current === 'string') {
          setApplicationVersion(response.data.current);
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <>
      <div className="text-neutral-400">{t('settings.about.information')}</div>

      <div className="mt-5 flex w-full flex-col space-y-5">
        <InfoRow label="Platform" value={capabilities?.platform.board || '-'} />
        <InfoRow
          label="Inventory backend"
          value={inventory?.backend ? inventory.backend.replace(/_/g, ' ') : '-'}
        />
        {inventory?.backend === 'kvmd_transition' && (
          <InfoRow label="KVMD" value={inventory.versions?.kvmd || '-'} />
        )}
        {inventory?.versions?.streamer && (
          <InfoRow label="Streamer" value={inventory.versions.streamer} />
        )}
        <InfoRow
          label="Video source"
          value={inventory?.video?.online === true ? 'online' : inventory ? 'not online' : '-'}
        />
        <InfoRow
          label="SoC temperature"
          value={formatTemperatureMillidegrees(inventory?.thermal?.temperatureMillidegreesCelsius)}
        />
        <InfoRow label="External RTC" value={formatRtcUnixSeconds(inventory?.rtc?.unixSeconds)} />
        <InfoRow
          label="RTC offset"
          value={formatRtcOffsetSeconds(inventory?.rtc?.offsetFromSystemSeconds)}
        />
        <InfoRow label={t('settings.about.mdns')} value={mdns || '-'} />
        <InfoRow label={t('settings.about.uptime')} value={formatUptime(uptime)} />
        <InfoRow
          label={t('settings.about.application')}
          value={runningSlot || formatHardenedVersion(applicationVersion)}
        />
      </div>
    </>
  );
};

/** Days, hours and minutes, as the reference shows the uptime. */
function formatUptime(seconds: number | null): string {
  if (seconds === null || seconds < 0) return '-';
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  return `${days}d ${hours}h ${minutes}m`;
}
