import { useEffect, useState } from 'react';
import { Input } from 'antd';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/vm.ts';

/**
 * Read-only hostname display. The hardened server exposes only a bounded
 * read-only `/api/hostname`; changing the hostname is a privileged system
 * action that stays out of the unprivileged server.
 */
export const Hostname = () => {
  const { t } = useTranslation();

  const [hostname, setHostname] = useState('');

  useEffect(() => {
    api
      .getHostname()
      .then((rsp) => {
        if (rsp.data?.hostname) {
          setHostname(rsp.data.hostname);
        }
      })
      .catch(() => undefined);
  }, []);

  return (
    <div className="space-y-1">
      <div className="flex w-full min-w-0 flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <span className="shrink-0">{t('settings.about.hostname')}</span>
        <Input readOnly className="min-w-0 sm:w-[180px]" value={hostname} />
      </div>
    </div>
  );
};
