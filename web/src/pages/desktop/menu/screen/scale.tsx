import { ReactElement, useEffect } from 'react';
import { useAtom } from 'jotai';
import { CheckIcon, PercentIcon, ScalingIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as storage from '@/lib/localstorage.ts';
import { videoScaleAtom } from '@/jotai/screen.ts';
import { MenuSubmenu } from '@/components/menu-submenu.tsx';

const ScaleList = [200, 150, 125, ...Array.from({ length: 19 }, (_, index) => 100 - index * 5)]
  .filter((percent) => percent >= 10)
  .map((percent) => ({ label: String(percent), value: percent / 100 }));

export const Scale = (): ReactElement => {
  const { t } = useTranslation();

  const [videoScale, setVideoScale] = useAtom(videoScaleAtom);

  useEffect(() => {
    const scale = storage.getVideoScale();
    if (scale) {
      setVideoScale(scale);
    }
  }, [setVideoScale]);

  async function update(scale: number): Promise<void> {
    setVideoScale(scale);
    storage.setVideoScale(scale);
  }

  const content = (
    <>
      {ScaleList.map((scale) => (
        <div
          key={scale.value}
          className="flex h-[30px] cursor-pointer select-none items-center rounded pl-1 pr-5 hover:bg-neutral-700/70"
          onClick={() => update(scale.value)}
        >
          <div className="flex h-[14px] w-[20px] items-end text-blue-500">
            {scale.value === videoScale && <CheckIcon size={14} />}
          </div>
          <div className="flex items-center space-x-0.5">
            <span>{scale.label}</span>
            <PercentIcon size={12} />
          </div>
        </div>
      ))}
    </>
  );

  return (
    <MenuSubmenu icon={<ScalingIcon size={16} />} label={t('screen.scale')} content={content} />
  );
};
