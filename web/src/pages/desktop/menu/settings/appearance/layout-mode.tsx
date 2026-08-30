import { Segmented } from 'antd';
import { useAtom } from 'jotai';
import { useTranslation } from 'react-i18next';

import * as storage from '@/lib/localstorage.ts';
import type { LayoutMode as LayoutModeValue } from '@/lib/localstorage.ts';
import { layoutModeAtom } from '@/jotai/settings.ts';

export const LayoutMode = () => {
  const { t } = useTranslation();
  const [layoutMode, setLayoutMode] = useAtom(layoutModeAtom);

  const options = [
    { value: 'auto', label: t('settings.appearance.layoutAuto') },
    { value: 'mobile', label: t('settings.appearance.layoutMobile') }
  ];

  function handleChange(mode: LayoutModeValue) {
    if (mode === layoutMode) return;

    setLayoutMode(mode);
    storage.setLayoutMode(mode);
  }

  return (
    <div className="mt-5 flex w-full min-w-0 flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
      <div className="flex min-w-0 flex-col space-y-1">
        <span>{t('settings.appearance.layoutMode')}</span>
        <span className="text-xs text-neutral-500">{t('settings.appearance.layoutModeDesc')}</span>
      </div>

      <div className="w-full min-w-0 overflow-x-auto sm:w-[340px] sm:flex-none">
        <Segmented
          block
          className="settings-segmented-wrap-labels min-w-full"
          value={layoutMode}
          options={options}
          onChange={(value) => handleChange(value as LayoutModeValue)}
        />
      </div>
    </div>
  );
};
