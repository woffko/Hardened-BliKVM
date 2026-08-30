import { MousePointer2Icon, TabletSmartphoneIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useIsTouchLayout } from '@/hooks/useResponsiveLayout.ts';

export const MobilePointerMode = () => {
  const { t } = useTranslation();
  const isTouchLayout = useIsTouchLayout();

  if (!isTouchLayout) {
    return null;
  }

  return (
    <div
      data-menu-keep-open
      className="flex min-h-[48px] items-start rounded px-3 py-2 text-neutral-300"
    >
      <div className="mr-2 mt-0.5 flex h-[18px] w-[18px] shrink-0 items-center justify-center text-blue-400">
        <TabletSmartphoneIcon size={18} />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5 text-sm text-neutral-100">
          <MousePointer2Icon size={15} />
          <span>{t('mouse.mobile.touchSync')}</span>
        </div>
        <div className="mt-0.5 whitespace-normal text-xs leading-4 text-neutral-500">
          {t('mouse.mobile.touchSyncDesc')}
        </div>
      </div>
    </div>
  );
};
