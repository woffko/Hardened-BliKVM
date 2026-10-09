import { useEffect, useRef, useState } from 'react';
import clsx from 'clsx';
import { useAtom, useAtomValue, useSetAtom } from 'jotai';

import { stopFrameDetect } from '@/api/stream.ts';
import { getFrameDetect } from '@/lib/localstorage.ts';
import * as storage from '@/lib/localstorage.ts';
import { pictureAttributes } from '@/lib/pointer-map.ts';
import { getBaseUrl } from '@/lib/service.ts';
import { checkSession } from '@/lib/session-check.ts';
import { client } from '@/lib/websocket.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { mouseStyleAtom } from '@/jotai/mouse.ts';
import {
  resolutionAtom,
  upscaleStoppedAtom,
  videoScaleAtom,
  videoStreamGenerationAtom
} from '@/jotai/screen.ts';
import { useScreenRef } from '@/hooks/useScreenRef.ts';
import { useUpscaleView } from '@/hooks/useUpscale.ts';

import { UpscaledMjpeg } from './upscaled-mjpeg.tsx';
import { useScreenFit } from './use-screen-fit.ts';

export const Mjpeg = () => {
  const [resolution, setResolution] = useAtom(resolutionAtom);
  const capabilities = useAtomValue(capabilitiesAtom);
  const isWritable = capabilities?.video.status.availability === 'read_write';
  const mouseStyle = useAtomValue(mouseStyleAtom);
  const [videoScale, setVideoScale] = useAtom(videoScaleAtom);
  const streamGeneration = useAtomValue(videoStreamGenerationAtom);
  const [hasError, setHasError] = useState(false);
  const [streamNonce, setStreamNonce] = useState(0);
  const [retries, setRetries] = useState(0);
  const streamInputs = useRef({ isWritable });
  const screenRef = useScreenRef<HTMLImageElement>();
  const streamURL = `${getBaseUrl('http')}/api/stream/mjpeg`;
  const streamSrc = hasError ? undefined : `${streamURL}?v=${streamNonce}`;
  const { upscale, fit } = useUpscaleView();
  const setUpscaleStopped = useSetAtom(upscaleStoppedAtom);
  const { containerRef, mediaStyle, drawn, pixelRatio } = useScreenFit(
    resolution,
    videoScale,
    {
      maxWidth: '100%',
      maxHeight: '100%',
      objectFit: 'scale-down'
    },
    fit
  );

  useEffect(() => {
    // stop frame detect for a while
    const enabled = getFrameDetect();
    if (isWritable && enabled) {
      stopFrameDetect(10);
    }
    setHasError(false);
    // The initial src already starts the stream. Replacing it on mount can
    // deadlock a full proxy: Chromium retains the old multipart image until
    // the replacement receives its first frame. Only a changed control mode
    // requires a new request; the resolution now follows the stream itself
    // and capture changes reload through the stream generation.
    const previous = streamInputs.current;
    streamInputs.current = { isWritable };
    if (previous.isWritable !== isWritable) {
      setStreamNonce((current) => current + 1);
    }
  }, [isWritable]);

  // An applied capture change restarts the streamer and silently ends this
  // response; request a new stream once the streamer is back.
  useEffect(() => {
    if (streamGeneration === 0) return;
    const timer = setTimeout(() => {
      setHasError(false);
      setRetries(0);
      setStreamNonce((current) => current + 1);
    }, 1500);
    return () => clearTimeout(timer);
  }, [streamGeneration]);

  // A stream that ended without an error event keeps showing its last frame;
  // request it again whenever the control socket had to reconnect.
  useEffect(
    () =>
      client.onReconnect(() => {
        setHasError(false);
        setStreamNonce((current) => current + 1);
      }),
    []
  );

  // A broken stream (server or proxy restart) is requested again after a
  // growing pause instead of staying hidden until the page is reloaded.
  useEffect(() => {
    if (!hasError) return;
    const delay = Math.min(2000 * 2 ** Math.min(retries, 3), 10000);
    const timer = setTimeout(() => {
      void checkSession().then((valid) => {
        if (!valid) return;
        setRetries((current) => current + 1);
        setStreamNonce((current) => current + 1);
        setHasError(false);
      });
    }, delay);
    return () => clearTimeout(timer);
  }, [hasError, retries]);

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
      {upscale !== 'off' ? (
        <UpscaledMjpeg
          url={streamSrc}
          sharpness={upscale}
          mediaStyle={mediaStyle}
          drawn={drawn}
          pixelRatio={pixelRatio}
          resolution={resolution}
          mouseStyle={mouseStyle}
          onPicture={(width, height) => {
            setRetries(0);
            if (width !== resolution?.width || height !== resolution?.height) {
              setResolution({ width, height });
              storage.setResolution({ width, height });
            }
          }}
          onError={() => setHasError(true)}
          onStopped={setUpscaleStopped}
        />
      ) : (
        <img
          id="screen"
          ref={screenRef}
          {...pictureAttributes(resolution)}
          className={clsx('m-auto block select-none', mouseStyle)}
          style={{
            ...mediaStyle,
            visibility: hasError ? 'hidden' : 'visible'
          }}
          src={streamSrc}
          onLoad={(event) => {
            setRetries(0);
            const { naturalWidth: width, naturalHeight: height } = event.currentTarget;
            // The frame size is the target's real resolution: the fit and the
            // pointer mapping must use it, not a stale stored value.
            if (
              width > 0 &&
              height > 0 &&
              (width !== resolution?.width || height !== resolution?.height)
            ) {
              setResolution({ width, height });
              storage.setResolution({ width, height });
            }
          }}
          onError={() => setHasError(true)}
          alt="screen"
        />
      )}
    </div>
  );
};
