import { ShieldOffIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useFocusConfirmation } from '@/hooks/useFocusConfirmation.ts';

/**
 * Gives remote control back. It is taken by the first press on the screen
 * (or a key) after a confirmation, and kept for the page's life; a window
 * that loses focus gives only the lease back.
 */
export const ReleaseControl = () => {
  const { t } = useTranslation();
  const { confirmed, disableControl } = useFocusConfirmation();
  if (!confirmed) return null;

  return (
    <div
      role="button"
      tabIndex={0}
      className="flex h-[30px] cursor-pointer select-none items-center space-x-2 rounded px-3 text-neutral-300 hover:bg-neutral-700/70"
      onClick={disableControl}
      onKeyDown={(event) => {
        if (event.key === 'Enter' || event.key === ' ') disableControl();
      }}
    >
      <ShieldOffIcon size={18} />
      <span>{t('mouse.releaseControl')}</span>
    </div>
  );
};
