import { Segmented, Tooltip } from 'antd';
import { useAtom } from 'jotai';
import { CheckIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as storage from '@/lib/localstorage.ts';
import { ACCENTS, type ThemeMode } from '@/lib/theme.ts';
import { accentAtom, themeModeAtom } from '@/jotai/settings.ts';

/** Dark, light or the system's theme, kept in this browser. */
export const Theme = () => {
  const { t } = useTranslation();
  const [themeMode, setThemeMode] = useAtom(themeModeAtom);

  const options = [
    { value: 'dark', label: t('settings.appearance.themeDark') },
    { value: 'light', label: t('settings.appearance.themeLight') },
    { value: 'system', label: t('settings.appearance.themeSystem') }
  ];

  function handleChange(mode: string) {
    if (mode === themeMode) return;
    setThemeMode(mode as ThemeMode);
    storage.setThemeMode(mode);
  }

  return (
    <div className="mt-5 flex w-full min-w-0 flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
      <div className="flex min-w-0 flex-col">
        <span className="text-neutral-400">{t('settings.appearance.theme')}</span>
        <span className="text-xs text-neutral-500">{t('settings.appearance.themeDesc')}</span>
      </div>

      <div className="w-full min-w-0 overflow-x-auto sm:w-[340px] sm:flex-none">
        <Segmented
          block
          className="settings-segmented-wrap-labels min-w-full"
          value={themeMode}
          options={options}
          onChange={handleChange}
        />
      </div>
    </div>
  );
};

/** The colour of buttons, switches and highlights, kept in this browser. */
export const Accent = () => {
  const { t } = useTranslation();
  const [accent, setAccent] = useAtom(accentAtom);

  function choose(name: string) {
    if (name === accent) return;
    setAccent(name);
    storage.setAccent(name);
  }

  return (
    <div className="mt-5 flex w-full min-w-0 flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
      <div className="flex min-w-0 flex-col">
        <span className="text-neutral-400">{t('settings.appearance.accent')}</span>
        <span className="text-xs text-neutral-500">{t('settings.appearance.accentDesc')}</span>
      </div>

      <div className="flex flex-wrap gap-2 sm:w-[340px] sm:flex-none sm:justify-end">
        {Object.entries(ACCENTS).map(([name, { shades }]) => (
          <Tooltip key={name} title={t(`settings.appearance.accents.${name}`)}>
            <button
              type="button"
              data-accent={name}
              aria-label={t(`settings.appearance.accents.${name}`)}
              aria-pressed={name === accent}
              className="flex h-7 w-7 items-center justify-center rounded-full border-2 border-neutral-700 text-[#fff]"
              style={{ backgroundColor: `rgb(${shades['500']})` }}
              onClick={() => choose(name)}
            >
              {name === accent && <CheckIcon size={14} />}
            </button>
          </Tooltip>
        ))}
      </div>
    </div>
  );
};
