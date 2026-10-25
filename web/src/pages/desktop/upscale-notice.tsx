import { useEffect } from 'react';
import { notification } from 'antd';
import { useAtomValue } from 'jotai';
import { useTranslation } from 'react-i18next';

import { upscaleStoppedAtom } from '@/jotai/screen.ts';

const NOTICE_KEY = 'upscale';

/**
 * Says once that the watchdog switched upscaling off for this page load
 * (docs/design/screen-upscaling-v1.md, "The safeguards"). A browser that
 * cannot upscale, or a lost context, goes on plainly without a word; the
 * Screen menu and the quality monitor say why in every case.
 */
export const UpscaleNotice = () => {
  const { t } = useTranslation();
  const [api, contextHolder] = notification.useNotification();
  const stopped = useAtomValue(upscaleStoppedAtom);

  useEffect(() => {
    if (stopped !== 'slow') return;
    api.warning({
      key: NOTICE_KEY,
      message: t('screen.upscaleSwitchedOff'),
      description: (
        <span data-testid="upscale-notice">
          {t('screen.upscaleStopped', { reason: t('screen.upscaleSlow') })}
        </span>
      ),
      placement: 'topRight',
      duration: 8
    });
  }, [api, stopped, t]);

  return <>{contextHolder}</>;
};
