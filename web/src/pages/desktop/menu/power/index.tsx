import { useCallback, useEffect, useState } from 'react';
import { Divider, Switch, Tooltip } from 'antd';
import clsx from 'clsx';
import { useAtomValue } from 'jotai';
import { LoaderCircleIcon, PowerIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/vm';
import { usesBliApplicationUpdates } from '@/lib/capabilities.ts';
import * as localstorage from '@/lib/localstorage.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { useCaseLeds } from '@/hooks/useCaseLeds.ts';
import { MenuItem } from '@/components/menu-item.tsx';

import { PowerLong } from './power-long.tsx';
import { PowerShort } from './power-short.tsx';
import { Reset } from './reset.tsx';

export const Power = () => {
  const { t } = useTranslation();
  const manifest = useAtomValue(capabilitiesAtom);

  const [legacyPowerOn, setLegacyPowerOn] = useState(false);
  const [isLoading, setIsLoading] = useState(false);
  const [showConfirm, setShowConfirm] = useState(false);
  // The hardened board answers its case LEDs on a light read; the target's
  // disk shows under the power icon (docs/baselines/work-plan-20261111-hdd-indicator.md).
  const hardened = usesBliApplicationUpdates(manifest);
  const leds = useCaseLeds(hardened);
  const isPowerOn = hardened ? leds?.powerLed === true : legacyPowerOn;
  const disk = hardened ? (leds?.hddLed ?? null) : null;

  const getLed = useCallback(async () => {
    // The hardened server has no legacy GPIO endpoint; never fall back to it.
    if (hardened) {
      return;
    }
    try {
      const rsp = await api.getGpio();
      if (rsp.code === 0) {
        setLegacyPowerOn(rsp.data.pwr);
      }
    } catch (err) {
      console.log(err);
    }
  }, [hardened]);

  useEffect(() => {
    getLed();
    const interval = setInterval(getLed, 5000);

    const powerConfirm = localstorage.getPowerConfirm();
    setShowConfirm(powerConfirm);

    return () => {
      clearInterval(interval);
    };
  }, [getLed]);

  function updateShowConfirm(value: boolean) {
    setShowConfirm(value);
    localstorage.setPowerConfirm(value);
  }

  const iconSize = disk === null ? 18 : 15;
  const icon = (
    <div
      className={clsx(
        'flex flex-col items-center',
        disk === null && 'h-[18px] w-[18px]',
        isPowerOn ? 'text-green-600' : 'text-neutral-300 hover:text-white'
      )}
    >
      {isLoading ? (
        <LoaderCircleIcon className="animate-spin" size={iconSize} />
      ) : (
        <PowerIcon size={iconSize} />
      )}
      {disk !== null && (
        <span
          data-hdd-led={disk ? 'active' : 'idle'}
          title={disk ? t('power.diskActive') : t('power.diskIdle')}
          className={clsx(
            'mt-[2px] rounded-[3px] px-[3px] text-[6.5px] font-extrabold leading-[9px] tracking-[0.06em]',
            disk ? 'bg-amber-500 text-stone-900' : 'bg-neutral-700 text-neutral-400'
          )}
        >
          HDD
        </span>
      )}
    </div>
  );

  const ledDot = (lit: boolean, color: string) => (
    <span
      aria-hidden="true"
      className={clsx('h-[9px] w-[9px] rounded-full', lit ? color : 'bg-neutral-600')}
    />
  );

  const content = (
    <div className="min-w-[200px]">
      <div className="flex items-center justify-between px-1">
        <span className="text-base font-bold text-neutral-300">{t('power.title')}</span>

        <div className="flex items-center space-x-2">
          <Tooltip title={t('power.showConfirmTip')} placement="right">
            <div className="flex items-center space-x-2">
              <span className="text-xs text-neutral-400">{t('power.showConfirm')}</span>

              <Switch size="small" checked={showConfirm} onChange={updateShowConfirm} />
            </div>
          </Tooltip>
        </div>
      </div>

      {leds && (leds.powerLed !== null || leds.hddLed !== null) && (
        <div data-testid="case-leds" className="flex items-center space-x-4 px-1 pt-2">
          {leds.powerLed !== null && (
            <span className="flex items-center space-x-2 text-sm text-neutral-300">
              {ledDot(leds.powerLed, 'bg-green-500')}
              <span>{leds.powerLed ? t('power.ledOn') : t('power.ledOff')}</span>
            </span>
          )}
          {leds.hddLed !== null && (
            <span className="flex items-center space-x-2 text-sm text-neutral-300">
              {ledDot(leds.hddLed, 'bg-amber-500')}
              <span>{t('power.ledDisk')}</span>
            </span>
          )}
        </div>
      )}

      <Divider style={{ margin: '10px 0 15px 0' }} />

      <div className="flex flex-col space-y-1">
        <Reset showConfirm={showConfirm} isLoading={isLoading} setIsLoading={setIsLoading} />
        <PowerShort showConfirm={showConfirm} isLoading={isLoading} setIsLoading={setIsLoading} />
        <PowerLong showConfirm={showConfirm} isLoading={isLoading} setIsLoading={setIsLoading} />
      </div>
    </div>
  );

  return <MenuItem title={t('power.title')} icon={icon} content={content} />;
};
