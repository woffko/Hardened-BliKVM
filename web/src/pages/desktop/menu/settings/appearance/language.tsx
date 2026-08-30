import { Select } from 'antd';
import { useTranslation } from 'react-i18next';

import languages from '@/i18n/languages.ts';
import { setLanguage } from '@/lib/localstorage.ts';

export const Language = () => {
  const { t, i18n } = useTranslation();

  const options = languages.map((language) => ({
    value: language.key,
    label: language.name
  }));

  function changeLanguage(value: string) {
    if (i18n.language === value) return;

    i18n.changeLanguage(value);
    setLanguage(value);
  }

  return (
    <div className="mt-5 flex min-w-0 flex-col gap-3 sm:flex-row sm:items-center sm:justify-between sm:gap-5">
      <div className="flex min-w-0 flex-col space-y-1">
        <span>{t('settings.appearance.language')}</span>
        <span className="text-xs text-neutral-500">{t('settings.appearance.languageDesc')}</span>
      </div>

      <div className="w-full sm:w-auto">
        <Select
          defaultValue={i18n.language}
          className="w-full sm:w-[180px]"
          options={options}
          onSelect={changeLanguage}
        />
      </div>
    </div>
  );
};
