import { useEffect, useState } from 'react';
import { useAtomValue } from 'jotai';
import { MonitorIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { updateScreen } from '@/api/vm';
import * as ls from '@/lib/localstorage';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { MenuItem } from '@/components/menu-item.tsx';

import { QualityMap } from './constants.ts';
import { Fps } from './fps';
import { Quality } from './quality';
import { Scale } from './scale';

export const Screen = () => {
  const { t } = useTranslation();

  const capabilities = useAtomValue(capabilitiesAtom);
  const isWritable = capabilities?.video.status.availability === 'read_write';
  // Only the entries the server enforces are offered: the MJPEG engine has no
  // video-mode, resolution, GOP or frame-detect switch.
  const videoSettings = capabilities?.video.settings ?? [];
  const [fps, setFps] = useState(30);
  const [quality, setQuality] = useState(2);

  useEffect(() => {
    if (!isWritable) return;

    const cookieQuality = ls.getQuality();
    if (cookieQuality) {
      const key = cookieQuality >= 1 && cookieQuality <= 4 ? cookieQuality : 2;
      const value = QualityMap.get(key)!;
      void updateScreen('quality', value).then((rsp) => {
        if (rsp.code === 0) setQuality(key);
      });
    }

    const cookieFps = ls.getFps();
    if (cookieFps) {
      void updateScreen('fps', cookieFps).then((rsp) => {
        if (rsp.code === 0) setFps(cookieFps);
      });
    }
  }, [isWritable]);

  const content = (
    <div className="flex flex-col space-y-1">
      {isWritable && videoSettings.includes('quality') && (
        <Quality quality={quality} setQuality={setQuality} />
      )}
      {isWritable && videoSettings.includes('fps') && <Fps fps={fps} setFps={setFps} />}
      <Scale />
    </div>
  );

  return <MenuItem title={t('screen.title')} icon={<MonitorIcon size={18} />} content={content} />;
};
