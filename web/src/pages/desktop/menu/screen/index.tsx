import { useEffect, useState } from 'react';
import { useAtomValue } from 'jotai';
import { MonitorIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { getScreen, updateScreen } from '@/api/vm';
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
  const [capturedFps, setCapturedFps] = useState<number | undefined>(undefined);
  const [delivered, setDelivered] = useState<{
    width?: number;
    height?: number;
    queuedFps?: number;
  }>({});

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

    // The bridge may deliver fewer frames than requested; report the measured
    // rate and frame size instead of pretending the requested values are what
    // the stream shows.
    void getScreen().then((rsp: any) => {
      const data = rsp?.data ?? {};
      if (typeof data.capturedFps === 'number') setCapturedFps(data.capturedFps);
      setDelivered({
        width: typeof data.width === 'number' ? data.width : undefined,
        height: typeof data.height === 'number' ? data.height : undefined,
        queuedFps: typeof data.queuedFps === 'number' ? data.queuedFps : undefined
      });
    });
  }, [isWritable]);

  const content = (
    <div className="flex flex-col space-y-1">
      {isWritable && videoSettings.includes('quality') && (
        <Quality quality={quality} setQuality={setQuality} />
      )}
      {isWritable && videoSettings.includes('fps') && (
        <Fps
          fps={fps}
          setFps={setFps}
          capturedFps={capturedFps}
          deliveredWidth={delivered.width}
          deliveredHeight={delivered.height}
          queuedFps={delivered.queuedFps}
        />
      )}
      <Scale />
    </div>
  );

  return <MenuItem title={t('screen.title')} icon={<MonitorIcon size={18} />} content={content} />;
};
