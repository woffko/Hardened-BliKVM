import { ReactElement, useEffect, useState } from 'react';
import { useAtom, useAtomValue, useSetAtom } from 'jotai';
import { CheckIcon, ScanSearchIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import {
  targetSize,
  writeUpscale,
  type Sharpness,
  type UpscaleSetting
} from '@/lib/upscale/settings.ts';
import type { UpscaleStop } from '@/lib/upscale/watchdog.ts';
import { drawsWithGpu } from '@/lib/vp8-range.ts';
import { isKeyboardEnableAtom } from '@/jotai/keyboard.ts';
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
const STOPPED: Record<UpscaleStop, string> = {
  unavailable: 'screen.upscaleUnavailable',
  lost: 'screen.upscaleLost',
  slow: 'screen.upscaleSlow'
};

// Upscaling of the picture in this browser (docs/design/screen-upscaling-v1.md):
// Off or Auto, how sharp, and how large an upscaled picture is shown (as
// large as the window, or at the target's own resolution); kept by the
// browser.
export const Upscale = (): ReactElement => {
  const { t } = useTranslation();
  const [setting, setSetting] = useAtom(upscaleAtom);
  const stopped = useAtomValue(upscaleStoppedAtom);
  const [gpu] = useState(() => drawsWithGpu());
  // The target's resolution as typed, until it is applied.
  const [typed, setTyped] = useState(() => ({
    width: String(setting.target?.width ?? 2560),
    height: String(setting.target?.height ?? 1440)
  }));
  const typedTarget = targetSize({ width: Number(typed.width), height: Number(typed.height) });
  // While the target's resolution is typed, the keys are not the target's:
  // the page stops sending them on, as for every field of its own.
  const setKeyboardEnabled = useSetAtom(isKeyboardEnableAtom);
  const [typing, setTyping] = useState(false);
  useEffect(() => {
    if (!typing) return;
    setKeyboardEnabled(false);
    return () => setKeyboardEnabled(true);
  }, [typing, setKeyboardEnabled]);

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
      <div className="px-2 pt-2 text-xs text-neutral-400">{t('screen.upscaleSize')}</div>
      {entry('fill', setting.size === 'fill', t('screen.upscaleFill'), () =>
        choose({ ...setting, size: 'fill' })
      )}
      {entry(
        'target',
        setting.size === 'target',
        setting.target
          ? `${t('screen.upscaleTarget')}: ${setting.target.width}×${setting.target.height}`
          : t('screen.upscaleTarget'),
        () => typedTarget && choose({ ...setting, size: 'target', target: typedTarget })
      )}
      <form
        className="flex items-center gap-1 px-2 py-1"
        onSubmit={(event) => {
          event.preventDefault();
          if (typedTarget) choose({ ...setting, size: 'target', target: typedTarget });
        }}
      >
        <input
          aria-label={t('screen.upscaleTargetWidth')}
          data-upscale-target-width
          className="w-[64px] rounded bg-neutral-800 px-1 text-sm text-neutral-200"
          inputMode="numeric"
          value={typed.width}
          onFocus={() => setTyping(true)}
          onBlur={() => setTyping(false)}
          onChange={(event) => setTyped({ ...typed, width: event.target.value })}
        />
        <span className="text-neutral-400">×</span>
        <input
          aria-label={t('screen.upscaleTargetHeight')}
          data-upscale-target-height
          className="w-[64px] rounded bg-neutral-800 px-1 text-sm text-neutral-200"
          inputMode="numeric"
          value={typed.height}
          onFocus={() => setTyping(true)}
          onBlur={() => setTyping(false)}
          onChange={(event) => setTyped({ ...typed, height: event.target.value })}
        />
        <button
          type="submit"
          disabled={!typedTarget}
          className="rounded px-2 text-sm text-blue-400 hover:bg-neutral-700/70 disabled:text-neutral-500"
        >
          {t('screen.upscaleApply')}
        </button>
      </form>
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
