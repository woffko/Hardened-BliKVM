import { useEffect, useRef, useState } from 'react';
import clsx from 'clsx';
import { useAtom, useAtomValue } from 'jotai';

import { stopFrameDetect } from '@/api/stream.ts';
import { getFrameDetect } from '@/lib/localstorage.ts';
import * as storage from '@/lib/localstorage.ts';
import { getBaseUrl } from '@/lib/service.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { mouseStyleAtom } from '@/jotai/mouse.ts';
import { resolutionAtom, videoScaleAtom } from '@/jotai/screen.ts';

import { useScreenFit } from './use-screen-fit.ts';

export const Mjpeg = () => {
  const resolution = useAtomValue(resolutionAtom);
  const capabilities = useAtomValue(capabilitiesAtom);
  const isWritable = capabilities?.video.status.availability === 'read_write';
  const mouseStyle = useAtomValue(mouseStyleAtom);
  const [videoScale, setVideoScale] = useAtom(videoScaleAtom);
  const [hasError, setHasError] = useState(false);
  const [streamNonce, setStreamNonce] = useState(0);
  const streamInputs = useRef({ isWritable, resolution });
  const streamURL = `${getBaseUrl('http')}/api/stream/mjpeg`;
  const streamSrc = hasError ? undefined : `${streamURL}?v=${streamNonce}`;
  const { containerRef, mediaStyle } = useScreenFit(resolution, videoScale, {
    maxWidth: '100%',
    maxHeight: '100%',
    objectFit: 'scale-down'
  });

  useEffect(() => {
    // stop frame detect for a while
    const enabled = getFrameDetect();
    if (isWritable && enabled) {
      stopFrameDetect(10);
    }
    setHasError(false);
    // The initial src already starts the stream. Replacing it on mount can
    // deadlock a full proxy: Chromium retains the old multipart image until
    // the replacement receives its first frame. Only changed inputs require
    // a new request (including a newly selected resolution object).
    const previous = streamInputs.current;
    streamInputs.current = { isWritable, resolution };
    if (previous.isWritable !== isWritable || previous.resolution !== resolution) {
      setStreamNonce((current) => current + 1);
    }
  }, [isWritable, resolution]);

  useEffect(() => {
    const scale = storage.getVideoScale();
    if (scale) {
      setVideoScale(scale);
    }
  }, [setVideoScale]);

  return (
    <div
      ref={containerRef}
      data-kvm-screen-scroll
      className="flex h-full min-h-0 w-full min-w-0 items-start justify-start overflow-auto overscroll-contain bg-black"
    >
      <img
        id="screen"
        className={clsx('m-auto block select-none', mouseStyle)}
        style={{
          ...mediaStyle,
          visibility: hasError ? 'hidden' : 'visible'
        }}
        src={streamSrc}
        onError={() => setHasError(true)}
        alt="screen"
      />
    </div>
  );
};
