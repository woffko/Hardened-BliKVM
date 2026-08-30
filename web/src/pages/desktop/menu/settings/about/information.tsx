import { useEffect, useState } from 'react';
import type { ReactNode } from 'react';
import { Tooltip } from 'antd';
import { CircleHelpIcon, EthernetPortIcon, WifiIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/vm.ts';
import { formatHardenedVersion } from '@/lib/hardened.ts';

import { Hostname } from './hostname.tsx';

type IP = {
  name: string;
  addr: string;
  version: string;
  type: string;
};

type Info = {
  ips: IP[];
  mdns: string;
  image: string;
  application: string;
  uptime?: number;
  deviceKey: string;
};

function formatUptime(seconds?: number) {
  if (!seconds || seconds < 0) return '-';

  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);

  if (days > 0) return `${days}d ${hours}h ${minutes}m`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}

type InfoRowProps = {
  label: ReactNode;
  value: ReactNode;
  alignStart?: boolean;
};

const InfoRow = ({ label, value, alignStart }: InfoRowProps) => (
  <div
    className={`flex w-full min-w-0 flex-col gap-2 sm:flex-row sm:justify-between ${
      alignStart ? 'sm:items-start' : 'sm:items-center'
    }`}
  >
    <div className="min-w-0 shrink-0 text-neutral-300 sm:max-w-[45%]">{label}</div>
    <div className="min-w-0 break-words text-left text-neutral-300 sm:text-right">{value}</div>
  </div>
);

export const Information = () => {
  const { t } = useTranslation();

  const [information, setInformation] = useState<Info>();

  useEffect(() => {
    api.getInfo().then((rsp: any) => {
      if (rsp.code !== 0) {
        console.log(rsp.msg);
        return;
      }

      setInformation(rsp.data);
    });
  }, []);

  return (
    <>
      <div className="text-neutral-400">{t('settings.about.information')}</div>

      <div className="mt-5 flex w-full flex-col space-y-5">
        {/* IP list */}
        <InfoRow
          alignStart
          label={<span>{t('settings.about.ip')}</span>}
          value={
            information?.ips && information.ips.length > 0 ? (
              <div className="flex min-w-0 flex-col space-y-1">
                {information.ips.map((ip) => (
                  <div
                    key={ip.addr}
                    className="flex min-w-0 items-center justify-start gap-2 sm:justify-end"
                  >
                    <span className="min-w-0 break-all">{ip.addr}</span>
                    <div className="size-[16px] shrink-0 text-neutral-500">
                      {ip.type === 'Wireless' ? (
                        <WifiIcon size={16} />
                      ) : (
                        <EthernetPortIcon size={16} />
                      )}
                    </div>
                  </div>
                ))}
              </div>
            ) : (
              <span>-</span>
            )
          }
        />

        {/* mDNS */}
        {!!information?.mdns && (
          <InfoRow label={<span>{t('settings.about.mdns')}</span>} value={information.mdns} />
        )}

        {/* image version */}
        <InfoRow
          label={
            <div className="flex items-center space-x-2">
              <span>{t('settings.about.image')}</span>
              <Tooltip
                title={t('settings.about.imageTip')}
                className="cursor-pointer text-neutral-500"
                placement="right"
              >
                <CircleHelpIcon size={15} />
              </Tooltip>
            </div>
          }
          value={<span>{information ? information.image : '-'}</span>}
        />

        {/* application version */}
        <InfoRow
          label={
            <div className="flex items-center space-x-2">
              <span>{t('settings.about.application')}</span>
              <Tooltip
                title={t('settings.about.applicationTip')}
                className="cursor-pointer text-neutral-500"
                placement="right"
              >
                <CircleHelpIcon size={15} />
              </Tooltip>
            </div>
          }
          value={<span>{information ? formatHardenedVersion(information.application) : '-'}</span>}
        />

        {/* uptime */}
        <InfoRow
          label={<span>{t('settings.about.uptime')}</span>}
          value={<span>{formatUptime(information?.uptime)}</span>}
        />

        <Hostname />
      </div>
    </>
  );
};
