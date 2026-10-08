import { ReactElement, useState } from 'react';
import { useAtom, useAtomValue } from 'jotai';
import { CheckIcon, ScanSearchIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { writeUpscale, type Sharpness, type UpscaleSetting } from '@/lib/upscale/settings.ts';
import { drawsWithGpu } from '@/lib/vp8-range.ts';
import { upscaleAtom, upscaleStoppedAtom } from '@/jotai/screen.ts';
import { MenuSubmenu } from '@/components/menu-submenu.tsx';

const MODES: { value: UpscaleSetting['mode']; label: string }[] = [
  { value: 'off', label: 'screen.upscaleOff' },
  { value: 'auto', label: 'screen.upscaleAuto' }
];
const SHARPNESSES: { value: Sharpness; label: string }[] = [
  { value: 'low', label: 'screen.upscaleLow' },
  { value: 'medium', label: 'screen.upscaleMedium' },
  { value: 'high', label: 'screen.upscaleHigh' }
];
const STOPPED: Record<'unavailable' | 'lost' | 'slow', string> = {
  unavailable: 'screen.upscaleUnavailable',
  lost: 'screen.upscaleLost',
  slow: 'screen.upscaleSlow'
};

// Upscaling of the picture in this browser (docs/design/screen-upscaling-v1.md):
// Off or Auto, and how sharp; kept by the browser.
export const Upscale = (): ReactElement => {
  const { t } = useTranslation();
  const [setting, setSetting] = useAtom(upscaleAtom);
  const stopped = useAtomValue(upscaleStoppedAtom);
  const [gpu] = useState(() => drawsWithGpu());

  function choose(next: UpscaleSetting) {
    setSetting(next);
    writeUpscale(window.localStorage, next);
  }

  const entry = (key: string, chosen: boolean, label: string, onClick: () => void) => (
    <div
      key={key}
      data-upscale-entry={key}
      className="flex h-[30px] cursor-pointer select-none items-center rounded pl-1 pr-5 hover:bg-neutral-700/70"
      onClick={onClick}
    >
      <div className="flex h-[14px] w-[20px] items-end text-blue-500">
        {chosen && <CheckIcon size={14} />}
      </div>
      <span>{label}</span>
    </div>
  );

  const note = !gpu
    ? t('screen.upscaleNoGpu')
    : stopped
      ? t('screen.upscaleStopped', { reason: t(STOPPED[stopped]) })
      : t('screen.upscaleTip');

  const content = (
    <div className="flex flex-col">
      {MODES.map((mode) =>
        entry(mode.value, setting.mode === mode.value, t(mode.label), () =>
          choose({ ...setting, mode: mode.value })
        )
      )}
      <div className="px-2 pt-2 text-xs text-neutral-400">{t('screen.upscaleSharpness')}</div>
      {SHARPNESSES.map((sharpness) =>
        entry(sharpness.value, setting.sharpness === sharpness.value, t(sharpness.label), () =>
          choose({ ...setting, sharpness: sharpness.value })
        )
      )}
      <div data-upscale-note className="max-w-[260px] px-2 pb-1 pt-2 text-xs text-neutral-400">
        {note}
      </div>
    </div>
  );

  return (
    <MenuSubmenu
      icon={<ScanSearchIcon size={16} />}
      label={t('screen.upscale')}
      content={content}
    />
  );
};
