import { useEffect, useState } from 'react';
import { Switch, Tooltip } from 'antd';
import { CircleAlertIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/vm.ts';

export const Mdns = ({ readOnly = false }: { readOnly?: boolean }) => {
  const { t } = useTranslation();

  const [isEnabled, setIsEnabled] = useState(false);
  const [isLoading, setIsLoading] = useState(true);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    let cancelled = false;
    setIsLoading(true);

    api
      .getMdnsState()
      .then((rsp) => {
        if (cancelled) return;
        if (rsp.code !== 0 || typeof rsp.data?.enabled !== 'boolean')
          throw new Error(rsp.msg || 'Failed to read mDNS state');
        setIsEnabled(rsp.data.enabled);
        setLoaded(true);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(err instanceof Error ? err.message : 'Failed to read mDNS state');
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function update() {
    if (readOnly || isLoading || !loaded) return;
    setIsLoading(true);
    setError('');
    const requested = !isEnabled;
    try {
      const rsp = requested ? await api.enableMdns() : await api.disableMdns();
      if (rsp.code !== 0 || rsp.data?.applied !== true || rsp.data?.rejection != null)
        throw new Error(rsp.data?.rejection || rsp.msg || 'Failed to update mDNS');
      const observed = await api.getMdnsState();
      if (observed.code !== 0 || typeof observed.data?.enabled !== 'boolean') {
        setLoaded(false);
        throw new Error('Failed to read mDNS state after update');
      }
      setIsEnabled(observed.data.enabled);
      if (observed.data.enabled !== requested)
        throw new Error('mDNS read-back did not match the requested state');
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to update mDNS');
    } finally {
      setIsLoading(false);
    }
  }

  return (
    <div className="flex items-center justify-between">
      <div className="flex flex-col space-y-1">
        <div className="flex items-center space-x-2">
          <span>mDNS</span>

          <Tooltip
            title={t('settings.device.mdns.tip')}
            className="cursor-pointer"
            placement="right"
            styles={{ root: { maxWidth: '400px' } }}
          >
            <CircleAlertIcon className="text-neutral-500" size={14} />
          </Tooltip>
        </div>

        <span className="text-xs text-neutral-500">{t('settings.device.mdns.description')}</span>
        {error && (
          <span role="alert" className="text-xs text-red-400">
            {error}
          </span>
        )}
      </div>

      <Switch
        aria-label="mDNS"
        checked={isEnabled}
        loading={isLoading}
        disabled={readOnly || !loaded}
        onChange={update}
      />
    </div>
  );
};
