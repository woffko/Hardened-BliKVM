import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import type { RuntimeInventory } from '@/types/inventory.ts';
import { useAtomValue } from 'jotai';
import { useTranslation } from 'react-i18next';

import { getCurrentVersion } from '@/api/application.ts';
import { getRuntimeInventory } from '@/api/inventory.ts';
import { formatHardenedVersion } from '@/lib/hardened.ts';
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

  useEffect(() => {
    let cancelled = false;
    getRuntimeInventory()
      .then((response) => {
        if (!cancelled && response.code === 0) setInventory(response.data);
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
        <InfoRow label="KVMD" value={inventory?.versions.kvmd || '-'} />
        <InfoRow label="Streamer" value={inventory?.versions.streamer || '-'} />
        <InfoRow
          label="Video source"
          value={inventory?.video.online === true ? 'online' : inventory ? 'not online' : '-'}
        />
        <InfoRow
          label="SoC temperature"
          value={formatTemperatureMillidegrees(inventory?.thermal.temperatureMillidegreesCelsius)}
        />
        <InfoRow
          label={t('settings.about.application')}
          value={formatHardenedVersion(applicationVersion)}
        />
      </div>
    </>
  );
};
