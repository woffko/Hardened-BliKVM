import { useEffect, useState } from 'react';
import { Select, Switch } from 'antd';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/vm.ts';

export const MouseJiggler = ({ readOnly = false }: { readOnly?: boolean }) => {
  const { t } = useTranslation();

  const [enabled, setEnabled] = useState(false);
  const [mode, setMode] = useState('relative');
  const [isLoading, setIsLoading] = useState(true);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    let cancelled = false;
    api
      .getMouseJiggler()
      .then((rsp) => {
        if (cancelled) return;
        if (
          rsp.code !== 0 ||
          typeof rsp.data?.enabled !== 'boolean' ||
          rsp.data?.mode !== 'relative'
        )
          throw new Error('Failed to read supported mouse jiggler state');
        setEnabled(rsp.data.enabled);
        setMode(rsp.data.mode);
        setLoaded(true);
      })
      .catch((err: unknown) => {
        if (!cancelled)
          setError(err instanceof Error ? err.message : 'Failed to read mouse jiggler state');
      })
      .finally(() => {
        if (!cancelled) setIsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // The absolute mode needs the pointer position the target never reports, so
  // only the reviewed relative nudge is offered.
  const options = [
    { value: 'disable', label: t('settings.device.mouseJiggler.disable') },
    { value: 'relative', label: t('settings.device.mouseJiggler.relative') }
  ];

  async function updateMode(value: string) {
    if (readOnly || isLoading || !loaded || !['disable', 'relative'].includes(value)) return;
    setIsLoading(true);
    setError('');
    const requested = value !== 'disable';
    try {
      const rsp = await api.setMouseJiggler(requested, 'relative');
      if (rsp.code !== 0 || rsp.data?.applied !== true || rsp.data?.rejection != null)
        throw new Error(rsp.data?.rejection || rsp.msg || 'Failed to update mouse jiggler');
      const observed = await api.getMouseJiggler();
      if (
        observed.code !== 0 ||
        typeof observed.data?.enabled !== 'boolean' ||
        observed.data?.mode !== 'relative'
      ) {
        setLoaded(false);
        throw new Error('Failed to read mouse jiggler state after update');
      }
      setEnabled(observed.data.enabled);
      setMode(observed.data.mode);
      if (observed.data.enabled !== requested)
        throw new Error('Mouse jiggler read-back did not match the requested state');
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to update mouse jiggler');
    } finally {
      setIsLoading(false);
    }
  }

  return (
    <div className="flex items-center justify-between">
      <div className="flex flex-col space-y-1">
        <span>{t('settings.device.mouseJiggler.title')}</span>
        <span className="text-xs text-neutral-500">
          {t('settings.device.mouseJiggler.description')}
        </span>
        {error && (
          <span role="alert" className="text-xs text-red-400">
            {error}
          </span>
        )}
      </div>

      {enabled ? (
        <Select
          aria-label="Mouse jiggler mode"
          disabled={readOnly || !loaded || isLoading}
          style={{ width: 150 }}
          value={mode}
          options={options}
          loading={isLoading}
          onChange={updateMode}
        />
      ) : (
        <Switch
          aria-label="Mouse jiggler"
          disabled={readOnly || !loaded}
          checked={enabled}
          loading={isLoading}
          onChange={() => void updateMode('relative')}
        />
      )}
    </div>
  );
};
