import { useCallback, useEffect, useRef, useState } from 'react';
import { useAtomValue } from 'jotai';
import { MonitorIcon } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { getScreen } from '@/api/vm';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { MenuItem } from '@/components/menu-item.tsx';

import { QualityMap, ResolutionList } from './constants.ts';
import { Fps } from './fps';
import { Quality } from './quality';
import { Resolution } from './resolution';
import { Scale } from './scale';

export const Screen = () => {
  const { t } = useTranslation();

  const capabilities = useAtomValue(capabilitiesAtom);
  const isWritable = capabilities?.video.status.availability === 'read_write';
  // Offer only settings explicitly backed by this server/owner configuration.
  const videoSettings = capabilities?.video.settings ?? [];
  const [fps, setFps] = useState(30);
  const [quality, setQuality] = useState(2);
  const [requestedResolution, setRequestedResolution] = useState<
    (typeof ResolutionList)[number] | undefined
  >();
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState('');
  const [capturedFps, setCapturedFps] = useState<number | undefined>(undefined);
  const [delivered, setDelivered] = useState<{
    width?: number;
    height?: number;
    queuedFps?: number;
  }>({});

  const mounted = useRef(false);
  const refresh = useCallback(async (): Promise<{
    fps: number;
    quality?: number;
    requestedResolution?: string;
  }> => {
    const rsp = await getScreen();
    if (!mounted.current) throw new Error('Screen settings closed');
    const data = rsp?.data ?? {};
    if (
      rsp.code !== 0 ||
      data.applied !== true ||
      data.rejection != null ||
      !Number.isInteger(data.fps) ||
      data.fps < 1 ||
      data.fps > 60
    )
      throw new Error('Unable to read video settings');
    setFps(data.fps);
    const resolution = ResolutionList.find(
      (item) => `${item.width}x${item.height}` === data.requestedResolution
    );
    setRequestedResolution(resolution);
    const qualityEntry = [...QualityMap.entries()].find(([, value]) => value === data.quality);
    setQuality(qualityEntry?.[0] ?? 0);
    const measured = (value: unknown) =>
      typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : undefined;
    setCapturedFps(measured(data.capturedFps));
    setDelivered({
      width: measured(data.width),
      height: measured(data.height),
      queuedFps: measured(data.queuedFps)
    });
    setLoaded(true);
    setError('');
    return {
      quality: typeof data.quality === 'number' ? data.quality : undefined,
      fps: data.fps,
      requestedResolution: resolution ? `${resolution.width}x${resolution.height}` : undefined
    };
  }, []);

  useEffect(() => {
    mounted.current = true;
    setLoaded(false);
    if (isWritable)
      void refresh().catch(() => {
        if (mounted.current) setError('Unable to read video settings');
      });
    return () => {
      mounted.current = false;
    };
  }, [isWritable, refresh]);

  const content = (
    <div className="flex flex-col space-y-1">
      {error && (
        <span role="alert" className="text-xs text-red-400">
          {error}
        </span>
      )}
      {isWritable && loaded && quality > 0 && videoSettings.includes('quality') && (
        <Quality quality={quality} refresh={refresh} />
      )}
      {isWritable && loaded && videoSettings.includes('fps') && (
        <Fps
          fps={fps}
          refresh={refresh}
          capturedFps={capturedFps}
          deliveredWidth={delivered.width}
          deliveredHeight={delivered.height}
          queuedFps={delivered.queuedFps}
        />
      )}
      {isWritable && loaded && videoSettings.includes('resolution') && requestedResolution && (
        <Resolution resolution={requestedResolution} refresh={refresh} />
      )}
      <Scale />
    </div>
  );

  return <MenuItem title={t('screen.title')} icon={<MonitorIcon size={18} />} content={content} />;
};
