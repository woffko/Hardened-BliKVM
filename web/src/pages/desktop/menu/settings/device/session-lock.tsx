import { useEffect, useState } from 'react';
import { Select } from 'antd';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/vm.ts';
import { getCsrfToken, setCsrfToken } from '@/lib/cookie.ts';

const DEFAULT_DURATION = '900';

export const SessionLock = ({ readOnly = false }: { readOnly?: boolean }) => {
  const { t } = useTranslation();

  const [isLoading, setIsLoading] = useState(true);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState('');
  const [duration, setDuration] = useState(DEFAULT_DURATION);

  const options = [
    { value: '300', label: t('settings.device.sessionLock.5') },
    { value: '900', label: t('settings.device.sessionLock.15') },
    { value: '1800', label: t('settings.device.sessionLock.30') },
    { value: '3600', label: t('settings.device.sessionLock.60') }
  ];

  useEffect(() => {
    getSessionLock();
  }, []);

  function getSessionLock() {
    setIsLoading(true);

    api
      .getSessionLock()
      .then((rsp) => {
        const value = rsp.data?.duration;
        if (rsp.code !== 0 || !Number.isInteger(value) || value < 0 || value > 86400)
          throw new Error(rsp.msg || 'Failed to read session lock duration');
        setDuration(String(value));
        setLoaded(true);
      })
      .catch((err) => {
        setError(err instanceof Error ? err.message : 'Failed to read session lock duration');
      })
      .finally(() => {
        setIsLoading(false);
      });
  }

  function update(value: string) {
    if (readOnly || isLoading || !loaded || !options.some((option) => option.value === value))
      return;
    setIsLoading(true);
    setError('');

    api
      .setSessionLock(parseInt(value))
      .then((rsp) => {
        if (rsp.code !== 0 || rsp.data?.duration !== Number(value))
          throw new Error(rsp.msg || 'Session lock read-back did not match the requested duration');

        setDuration(value);
        const expiresAt = rsp.data?.expiresAt;
        const csrfToken = getCsrfToken();
        if (expiresAt && csrfToken) {
          setCsrfToken(csrfToken, expiresAt);
        }
      })
      .catch((err) => {
        setError(err instanceof Error ? err.message : 'Failed to update session lock duration');
      })
      .finally(() => {
        setIsLoading(false);
      });
  }

  return (
    <div className="flex items-center justify-between">
      <div className="flex flex-col space-y-1">
        <span>{t('settings.device.sessionLock.title')}</span>
        <span className="text-xs text-neutral-500">
          {t('settings.device.sessionLock.description')}
        </span>
        {error && (
          <span role="alert" className="text-xs text-red-400">
            {error}
          </span>
        )}
      </div>

      <Select
        aria-label="Session lock duration"
        disabled={readOnly || !loaded || isLoading}
        style={{ width: 150 }}
        value={duration}
        options={options}
        loading={isLoading}
        onChange={update}
      />
    </div>
  );
};
