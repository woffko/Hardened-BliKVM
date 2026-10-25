import { useEffect, useState } from 'react';
import { Select } from 'antd';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/vm.ts';

/** The times the board's display may stay lit without activity; 0 is never. */
const CHOICES = [0, 15, 30, 60, 180, 300, 600, 1800, 3600];

/**
 * The board's display goes dark after a time without activity, as in
 * Hardened NanoKVM; the board's button lights it again
 * (docs/design/board-daemon-v1.md, "The display's sleep").
 */
export const Oled = ({ readOnly = false }: { readOnly?: boolean }) => {
  const { t } = useTranslation();

  const [isLoading, setIsLoading] = useState(true);
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState('');
  const [sleep, setSleep] = useState('0');

  const options = CHOICES.map((seconds) => ({
    value: seconds.toString(),
    label: t(`settings.device.oled.${seconds}`)
  }));

  useEffect(() => {
    api
      .getOLED()
      .then((rsp) => {
        const value = rsp.data?.sleep;
        if (rsp.code !== 0 || !rsp.data?.exist || !CHOICES.includes(value))
          throw new Error(rsp.msg || 'Failed to read the display setting');
        setSleep(String(value));
        setLoaded(true);
      })
      .catch((err) => {
        setError(err instanceof Error ? err.message : 'Failed to read the display setting');
      })
      .finally(() => {
        setIsLoading(false);
      });
  }, []);

  function update(value: string) {
    if (readOnly || isLoading || !loaded || !CHOICES.includes(Number(value))) return;
    setIsLoading(true);
    setError('');

    api
      .setOLED(Number(value))
      .then((rsp) => {
        if (rsp.code !== 0) throw new Error(rsp.msg || 'Failed to update the display setting');
        if (!rsp.data?.applied) throw new Error(rsp.data?.rejection || 'display_refused');
        setSleep(value);
      })
      .catch((err) => {
        setError(err instanceof Error ? err.message : 'Failed to update the display setting');
      })
      .finally(() => {
        setIsLoading(false);
      });
  }

  return (
    <div className="flex items-center justify-between">
      <div className="flex flex-col space-y-1">
        <span>{t('settings.device.oled.title')}</span>
        <span className="text-xs text-neutral-500">{t('settings.device.oled.description')}</span>
        {error && (
          <span role="alert" className="text-xs text-red-400">
            {error}
          </span>
        )}
      </div>

      <Select
        aria-label="Display sleep"
        disabled={readOnly || !loaded || isLoading}
        style={{ width: 150 }}
        value={sleep}
        options={options}
        loading={isLoading}
        onChange={update}
      />
    </div>
  );
};
