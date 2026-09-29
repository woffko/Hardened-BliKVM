import { useEffect, useRef, useState } from 'react';
import { Modal, Switch, Tooltip } from 'antd';
import { CircleAlertIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/vm.ts';

const REDIRECT_DELAY_SECONDS = 30;

export const Tls = ({ readOnly = false }: { readOnly?: boolean }) => {
  const { t } = useTranslation();

  const [isEnabled, setIsEnabled] = useState(false);
  const [isLoading, setIsLoading] = useState(true);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState('');
  const redirectTimer = useRef<number>();

  useEffect(() => {
    let cancelled = false;
    api
      .getTLS()
      .then((rsp) => {
        if (cancelled) return;
        if (rsp.code !== 0 || typeof rsp.data?.enabled !== 'boolean')
          throw new Error(rsp.msg || 'Failed to read HTTPS state');
        setIsEnabled(rsp.data.enabled);
        setLoaded(true);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(err instanceof Error ? err.message : 'Failed to read HTTPS state');
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });
    return () => {
      cancelled = true;
      window.clearTimeout(redirectTimer.current);
    };
  }, []);

  function requestUpdate() {
    if (readOnly || isLoading || !loaded) return;

    const enable = !isEnabled;
    const target = protocolRedirectUrl(enable);

    Modal.confirm({
      title: t('settings.network.tls.rebootTitle'),
      content: (
        <div className="space-y-2">
          <p>{t('settings.network.tls.rebootDesc', { seconds: REDIRECT_DELAY_SECONDS })}</p>
          <p className="break-all font-mono text-xs text-neutral-400">{target}</p>
        </div>
      ),
      okText: t('settings.network.tls.rebootOk'),
      cancelText: t('settings.network.tls.rebootCancel'),
      onOk: () => update(enable, target)
    });
  }

  async function update(enable: boolean, target: string) {
    if (readOnly || isLoading || !loaded) return;
    setIsLoading(true);
    setError('');

    try {
      const rsp = await api.setTLS(enable);
      if (rsp.code !== 0 || rsp.data?.applied !== true || rsp.data?.rejection != null)
        throw new Error(rsp.data?.rejection || rsp.msg || 'Failed to update HTTPS');
      setIsEnabled(enable);
      redirectTimer.current = window.setTimeout(() => {
        window.location.assign(target);
      }, REDIRECT_DELAY_SECONDS * 1000);
      return;
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to update HTTPS');
    }

    setIsLoading(false);
  }

  function protocolRedirectUrl(enable: boolean) {
    const target = new URL(window.location.href);
    target.protocol = enable ? 'https:' : 'http:';

    if (enable && target.port === '80') {
      target.port = '';
    }
    if (!enable && target.port === '443') {
      target.port = '';
    }

    return target.toString();
  }

  return (
    <div className="flex min-w-0 items-center justify-between gap-3">
      <div className="flex min-w-0 flex-col space-y-1">
        <div className="flex items-center space-x-2">
          <span>HTTPS</span>

          <Tooltip
            title={t('settings.network.tls.tip')}
            className="cursor-pointer"
            placement="right"
            styles={{ root: { maxWidth: '400px' } }}
          >
            <CircleAlertIcon className="text-neutral-500" size={14} />
          </Tooltip>
        </div>
        <span className="break-words text-xs text-neutral-500">
          {t('settings.network.tls.description')}
        </span>
        {readOnly && loaded && (
          <span data-testid="tls-always-on" className="break-words text-xs text-neutral-500">
            {t('settings.network.tls.alwaysOn')}
          </span>
        )}
        {error && (
          <span role="alert" className="text-xs text-red-400">
            {error}
          </span>
        )}
      </div>

      <Switch
        aria-label="HTTPS"
        disabled={readOnly || !loaded}
        className="shrink-0"
        checked={isEnabled}
        loading={isLoading}
        onChange={requestUpdate}
      />
    </div>
  );
};
