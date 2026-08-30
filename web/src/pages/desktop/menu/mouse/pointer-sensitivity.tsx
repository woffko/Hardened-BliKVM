import { Slider } from 'antd';
import { useAtom } from 'jotai';
import { MousePointer2Icon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as storage from '@/lib/localstorage.ts';
import { pointerSensitivityAtom } from '@/jotai/mouse.ts';
import { MenuSubmenu } from '@/components/menu-submenu.tsx';

const MIN_SENSITIVITY = 0.25;
const MAX_SENSITIVITY = 2;

export const PointerSensitivity = () => {
  const { t } = useTranslation();
  const [sensitivity, setSensitivity] = useAtom(pointerSensitivityAtom);

  function update(value: number): void {
    const next = Math.max(MIN_SENSITIVITY, Math.min(MAX_SENSITIVITY, value / 100));
    setSensitivity(next);
    storage.setPointerSensitivity(next);
  }

  const content = (
    <div className="min-w-[220px] px-2 py-3">
      <div className="mb-2 text-xs leading-4 text-neutral-500">{t('mouse.sensitivityDesc')}</div>
      <Slider
        marks={{
          25: <span>{t('mouse.slow')}</span>,
          100: <span>1x</span>,
          200: <span>{t('mouse.fast')}</span>
        }}
        min={25}
        max={200}
        step={25}
        value={Math.round(sensitivity * 100)}
        onChange={update}
      />
    </div>
  );

  return (
    <MenuSubmenu
      icon={<MousePointer2Icon size={18} />}
      label={t('mouse.sensitivity')}
      content={content}
    />
  );
};
