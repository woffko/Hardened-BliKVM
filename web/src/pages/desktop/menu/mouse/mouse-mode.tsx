import { useAtom } from 'jotai';
import { CheckIcon, SquareDashedMousePointerIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as ls from '@/lib/localstorage.ts';
import { client } from '@/lib/websocket.ts';
import { mouseModeAtom } from '@/jotai/mouse.ts';
import { MenuSubmenu } from '@/components/menu-submenu.tsx';

export const MouseMode = () => {
  const { t } = useTranslation();

  const [mouseMode, setMouseMode] = useAtom(mouseModeAtom);

  const mouseModes = [
    { name: t('mouse.absolute'), value: 'absolute' },
    { name: t('mouse.relative'), value: 'relative' }
  ];

  function updateMouseMode(mode: string) {
    setMouseMode(mode);
    ls.setMouseMode(mode);

    if (mode === 'relative') {
      client.close();
      setTimeout(() => {
        client.connect();
      }, 500);
    }
  }

  const content = (
    <>
      {mouseModes.map((mode) => (
        <div
          key={mode.value}
          className="flex cursor-pointer items-center space-x-1 rounded py-1.5 pl-2 pr-5 hover:bg-neutral-700/70"
          onClick={() => updateMouseMode(mode.value)}
        >
          <div className="flex h-[16px] w-[16px] items-end text-blue-500">
            {mode.value === mouseMode && <CheckIcon strokeWidth={3} size={16} />}
          </div>
          <span>{mode.name}</span>
        </div>
      ))}
    </>
  );

  return (
    <MenuSubmenu
      icon={<SquareDashedMousePointerIcon size={18} />}
      label={t('mouse.mode')}
      content={content}
    />
  );
};
