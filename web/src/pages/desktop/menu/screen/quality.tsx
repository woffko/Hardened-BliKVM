import { useRef } from 'react';
import { message } from 'antd';
import { useAtomValue } from 'jotai';
import { CheckIcon, SquareActivityIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { updateScreen } from '@/api/vm';
import { setQuality as setCookie } from '@/lib/localstorage.ts';
import { videoModeAtom } from '@/jotai/screen.ts';
import { MenuSubmenu } from '@/components/menu-submenu.tsx';

import { QualityMap } from './constants.ts';

type QualityProps = {
  quality: number;
  refresh: () => Promise<{ quality?: number }>;
};

export const Quality = ({ quality, refresh }: QualityProps) => {
  const { t } = useTranslation();
  const videoMode = useAtomValue(videoModeAtom);
  const updating = useRef(false);

  const qualityList = [
    { key: 1, label: t('screen.qualityLossless') },
    { key: 2, label: t('screen.qualityHigh') },
    { key: 3, label: t('screen.qualityMedium') },
    { key: 4, label: t('screen.qualityLow') }
  ];

  async function update(key: number) {
    const value = QualityMap.get(key);
    if (updating.current || value === undefined) return;
    if (videoMode !== 'mjpeg') {
      message.error('JPEG quality is only available for MJPEG');
      return;
    }
    updating.current = true;
    try {
      const rsp = await updateScreen('quality', value);
      if (rsp.code !== 0 || rsp.data?.applied !== true || rsp.data?.rejection != null)
        throw new Error(rsp.data?.rejection || 'Quality update was refused');
      if ((await refresh()).quality !== value)
        throw new Error('Quality read-back did not match the request');
      setCookie(key);
    } catch (err) {
      message.error(err instanceof Error ? err.message : 'Unable to update quality');
    } finally {
      updating.current = false;
    }
  }

  const content = (
    <>
      {qualityList.map((item) => (
        <div
          key={item.key}
          className="flex h-[30px] cursor-pointer select-none items-center rounded pl-1 pr-5 hover:bg-neutral-700/70"
          onClick={() => update(item.key)}
        >
          <div className="flex h-[14px] w-[20px] items-end text-blue-500">
            {item.key === quality && <CheckIcon size={14} />}
          </div>
          <span className="flex w-[50px]">{item.label}</span>
        </div>
      ))}
    </>
  );

  return (
    <MenuSubmenu
      icon={<SquareActivityIcon size={18} />}
      label={t('screen.quality')}
      content={content}
    />
  );
};
