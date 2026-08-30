import { Input } from 'antd';
import { useAtom } from 'jotai';
import { useTranslation } from 'react-i18next';

import { HARDENED_NAME } from '@/lib/hardened.ts';
import * as storage from '@/lib/localstorage.ts';
import { webTitleAtom } from '@/jotai/settings.ts';

export const WebTitle = () => {
  const { t } = useTranslation();
  const [webTitle, setWebTitle] = useAtom(webTitleAtom);

  function submit() {
    setWebTitle(storage.setWebTitle(webTitle));
  }

  return (
    <div className="mt-8 flex min-w-0 flex-col gap-3 sm:flex-row sm:items-center sm:justify-between sm:gap-5">
      <div className="flex min-w-0 flex-col">
        <span>{t('settings.appearance.webTitle')}</span>
        <span className="text-xs text-neutral-500">{t('settings.appearance.webTitleDesc')}</span>
      </div>

      <div className="w-full sm:w-auto">
        <Input
          className="w-full sm:w-[180px]"
          value={webTitle}
          onChange={(e) => setWebTitle(e.target.value)}
          onPressEnter={submit}
          onBlur={submit}
          placeholder={HARDENED_NAME}
        />
      </div>
    </div>
  );
};
