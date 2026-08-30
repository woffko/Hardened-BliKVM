import clsx from 'clsx';
import { useAtom } from 'jotai';
import { KeyboardIcon, SmartphoneIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { isLocalKeyboardOpenAtom } from '@/jotai/keyboard.ts';
import { useIsTouchLayout } from '@/hooks/useResponsiveLayout.ts';
import {
  blurMobileLocalKeyboard,
  focusMobileLocalKeyboard
} from '@/pages/desktop/mobile-local-keyboard/controls.ts';

export const LocalKeyboard = () => {
  const { t } = useTranslation();
  const isTouchLayout = useIsTouchLayout();
  const [isLocalKeyboardOpen, setIsLocalKeyboardOpen] = useAtom(isLocalKeyboardOpenAtom);

  if (!isTouchLayout) {
    return null;
  }

  function toggleLocalKeyboard() {
    if (isLocalKeyboardOpen) {
      setIsLocalKeyboardOpen(false);
      blurMobileLocalKeyboard();
      return;
    }

    setIsLocalKeyboardOpen(true);
    focusMobileLocalKeyboard();
  }

  return (
    <div
      className={clsx(
        'flex cursor-pointer select-none items-center space-x-2 rounded py-1 pl-2 pr-5 hover:bg-neutral-700/70',
        isLocalKeyboardOpen && 'text-blue-400'
      )}
      onClick={toggleLocalKeyboard}
    >
      <span className="relative flex h-[18px] w-[22px] items-center justify-center">
        <KeyboardIcon size={18} />
        <SmartphoneIcon size={10} className="absolute -right-1 -top-1 rounded bg-neutral-800" />
      </span>
      <span>{t('keyboard.local')}</span>
    </div>
  );
};
