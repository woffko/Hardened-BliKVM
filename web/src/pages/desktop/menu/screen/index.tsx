import { useEffect, useRef, useState } from 'react';
import { useAtomValue } from 'jotai';
import { MonitorIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { updateScreen } from '@/api/vm';
import * as ls from '@/lib/localstorage';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { resolutionAtom, videoModeAtom } from '@/jotai/screen.ts';
import { MenuItem } from '@/components/menu-item.tsx';

import { BitRateMap, QualityMap } from './constants.ts';
import { Fps } from './fps';
import { FrameDetect } from './frame-detect';
import { Gop } from './gop.tsx';
import { Quality } from './quality';
import { Reset } from './reset.tsx';
import { Resolution } from './resolution';
import { Scale } from './scale';
import { VideoMode } from './video-mode.tsx';

export const Screen = () => {
  const { t } = useTranslation();

  const videoMode = useAtomValue(videoModeAtom);
  const resolution = useAtomValue(resolutionAtom);
  const capabilities = useAtomValue(capabilitiesAtom);
  const isWritable = capabilities?.video.status.availability === 'read_write';
  const [fps, setFps] = useState(30);
  const [quality, setQuality] = useState(2);
  const [gop, setGop] = useState(30);
  const initialVideoModeRef = useRef(videoMode);
  const initialResolutionRef = useRef(resolution);

  useEffect(() => {
    if (!isWritable) return;
    const initialVideoMode = initialVideoModeRef.current;
    const initialResolution = initialResolutionRef.current;
    void updateScreen('type', initialVideoMode === 'mjpeg' ? 0 : 1);
    if (initialResolution) {
      void updateScreen('resolution', initialResolution.height);
    }

    const cookieQuality = ls.getQuality();
    if (cookieQuality) {
      const key = cookieQuality >= 1 && cookieQuality <= 4 ? cookieQuality : 2;
      const value = initialVideoMode === 'mjpeg' ? QualityMap.get(key)! : BitRateMap.get(key)!;
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

    const cookieGop = ls.getGop();
    if (cookieGop) {
      void updateScreen('gop', cookieGop).then((rsp) => {
        if (rsp.code === 0) setGop(cookieGop);
      });
    }
  }, [isWritable]);

  const content = (
    <div className="flex flex-col space-y-1">
      {isWritable && <VideoMode />}
      {isWritable && <Resolution />}
      {isWritable && <Quality quality={quality} setQuality={setQuality} />}
      {isWritable && <Fps fps={fps} setFps={setFps} />}
      <Scale />
      {isWritable && videoMode !== 'mjpeg' && <Gop gop={gop} setGop={setGop} />}
      {isWritable && videoMode === 'mjpeg' && <FrameDetect />}
      {isWritable && <Reset />}
    </div>
  );

  return <MenuItem title={t('screen.title')} icon={<MonitorIcon size={18} />} content={content} />;
};
