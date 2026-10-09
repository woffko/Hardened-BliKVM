import { useEffect, useRef, useState } from 'react';
import { Spin } from 'antd';
import clsx from 'clsx';
import { useAtom, useAtomValue, useSetAtom } from 'jotai';

import * as storage from '@/lib/localstorage.ts';
import { pictureAttributes } from '@/lib/pointer-map.ts';
import { quicSession } from '@/lib/quic/session.ts';
import type { FromVideoWorker, ToVideoWorker } from '@/lib/quic/video.worker.ts';
import VideoWorker from '@/lib/quic/video.worker.ts?worker';
import { mouseStyleAtom } from '@/jotai/mouse.ts';
import { resolutionAtom, upscaleStoppedAtom, videoScaleAtom } from '@/jotai/screen.ts';
import { quicVideoAtom, quicVideoGivenUpAtom, quicVideoStatsAtom } from '@/jotai/transport.ts';
import { useScreenRef } from '@/hooks/useScreenRef.ts';
import { useUpscaleView } from '@/hooks/useUpscale.ts';

import { useScreenFit } from './use-screen-fit.ts';

/**
 * A picture that shows no new frame for this long is not coming: even a
 * still screen gives one every two seconds. H.264 then goes on over WebRTC.
 */
const STALL_MS = 10000;
/**
 * The worker reports every second; a gap this long between two reports is
 * the browser holding the page or the worker (a phone's screen off), not a
 * picture that stalled.
 */
const HELD_MS = 3000;

/** The bytes of `bytes` as a buffer of their own, to hand to the worker. */
function buffer(bytes: Uint8Array): ArrayBuffer {
  const whole = bytes.byteOffset === 0 && bytes.byteLength === bytes.buffer.byteLength;
  return (whole ? bytes.buffer : bytes.slice().buffer) as ArrayBuffer;
}

// H.264 over the QUIC session: the session delivers datagrams and key-frame
// streams, a worker puts the frames together, decodes them with WebCodecs
// and draws them on this canvas, through the upscaler when upscaling is on
// (docs/design/screen-upscaling-v1.md).
export const H264Quic = () => {
  const [resolution, setResolution] = useAtom(resolutionAtom);
  const mouseStyle = useAtomValue(mouseStyleAtom);
  const [videoScale, setVideoScale] = useAtom(videoScaleAtom);
  const video = useAtomValue(quicVideoAtom);
  const setStats = useSetAtom(quicVideoStatsAtom);
  const setGivenUp = useSetAtom(quicVideoGivenUpAtom);
  const setUpscaleStopped = useSetAtom(upscaleStoppedAtom);
  const { upscale, fit } = useUpscaleView();
  // A canvas has one kind of context: drawing through the upscaler or
  // plainly is a canvas, and a worker, of its own.
  const upscaling = upscale !== 'off';
  const upscaleRef = useRef(upscale);
  upscaleRef.current = upscale;
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const screenRef = useScreenRef(canvasRef);
  const workerRef = useRef<Worker | null>(null);
  // A canvas can be handed to a worker once; a new one is mounted for the
  // next worker.
  const [canvasKey, setCanvasKey] = useState(0);
  const { containerRef, mediaStyle, settled, pixelRatio } = useScreenFit(
    resolution,
    videoScale,
    {
      maxWidth: '100%',
      maxHeight: '100%',
      objectFit: 'scale-down'
    },
    fit
  );
  // Where the picture is shown, for the worker to draw it at that size.
  const viewRef = useRef({ width: settled.width, height: settled.height, pixelRatio });
  viewRef.current = { width: settled.width, height: settled.height, pixelRatio };

  useEffect(() => {
    const scale = storage.getVideoScale();
    if (scale) setVideoScale(scale);
  }, [setVideoScale]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    let offscreen: OffscreenCanvas;
    try {
      offscreen = canvas.transferControlToOffscreen();
    } catch {
      // Handed over before, by an effect that has run twice.
      setCanvasKey((key) => key + 1);
      return;
    }
    const worker = new VideoWorker();
    workerRef.current = worker;
    const post = (message: ToVideoWorker, transfer: Transferable[] = []) =>
      worker.postMessage(message, transfer);
    post({ type: 'canvas', canvas: offscreen, upscale: upscaleRef.current }, [offscreen]);
    post({ type: 'view', ...viewRef.current });
    // A hidden page's picture is not drawn (docs/design/screen-upscaling-v1.md).
    const seen = () => post({ type: 'visible', visible: !document.hidden });
    seen();
    document.addEventListener('visibilitychange', seen);
    // The frames drawn at the last report, when one was last drawn, and when
    // the worker last reported.
    let frames = 0;
    let progressAt = performance.now();
    let reportedAt = performance.now();
    worker.onmessage = (event: MessageEvent<FromVideoWorker>) => {
      const message = event.data;
      switch (message.type) {
        case 'request':
          quicSession.requestKeyFrame(message.stream, message.lastFrame, message.dropped);
          break;
        case 'upscaler':
          // No WebGL2 here, or its context lost: the plain path for this
          // page load, on a canvas of its own.
          setUpscaleStopped(message.state);
          break;
        case 'picture':
          // The decoded frame's size is the target's real resolution.
          setResolution({ width: message.width, height: message.height });
          storage.setResolution({ width: message.width, height: message.height });
          break;
        case 'stats': {
          const at = performance.now();
          const held = at - reportedAt > HELD_MS;
          reportedAt = at;
          if (message.frames !== frames || !quicSession.isOpen() || document.hidden || held) {
            // A session that is lost is the session's to report; a hidden
            // or held page draws nothing and stalls nothing.
            frames = message.frames;
            progressAt = at;
          } else if (at - progressAt > STALL_MS) {
            setGivenUp('stalled');
          }
          setStats({
            frames: message.frames,
            framesPerSecond: message.framesPerSecond,
            frameRate: message.frameRate,
            ageMedianMs: message.ageMedianMs,
            ageP95Ms: message.ageP95Ms,
            waitMedianMs: message.waitMedianMs,
            dropped: message.dropped,
            requests: message.requests,
            resets: message.resets,
            queued: message.queued,
            bytes: message.bytes,
            received: message.received,
            decodeMedianMs: message.decodeMedianMs,
            upscale: message.upscale,
            canvasWidth: message.canvasWidth,
            canvasHeight: message.canvasHeight
          });
          // What a probe reads without opening Settings.
          canvas.dataset.quicFrames = String(message.frames);
          canvas.dataset.quicFps = String(message.framesPerSecond);
          canvas.dataset.quicRate = String(message.frameRate);
          canvas.dataset.quicAgeMedian = message.ageMedianMs?.toFixed(1) ?? '';
          canvas.dataset.quicAgeP95 = message.ageP95Ms?.toFixed(1) ?? '';
          canvas.dataset.quicWaitMedian = message.waitMedianMs?.toFixed(1) ?? '';
          canvas.dataset.quicDropped = String(message.dropped);
          canvas.dataset.quicRequests = String(message.requests);
          canvas.dataset.quicResets = String(message.resets);
          canvas.dataset.quicUpscale = message.upscale.mode;
          canvas.dataset.quicBuffer = `${message.canvasWidth}x${message.canvasHeight}`;
          const counted = message.upscale.frames;
          canvas.dataset.quicUpscaleFrames = counted
            ? `${counted.drawn.toFixed(1)}/${counted.received.toFixed(1)}`
            : '';
          break;
        }
      }
    };
    quicSession.watch({
      begin: () => {
        progressAt = performance.now();
        post({ type: 'begin' });
      },
      datagram: (bytes) => {
        const data = buffer(bytes);
        post({ type: 'datagram', data }, [data]);
      },
      keyFrame: (bytes) => {
        const data = buffer(bytes);
        post({ type: 'keyframe', data }, [data]);
      },
      pong: (roundTripMs, boardMicros, at) => post({ type: 'pong', roundTripMs, boardMicros, at })
    });
    return () => {
      document.removeEventListener('visibilitychange', seen);
      quicSession.unwatch();
      worker.terminate();
      if (workerRef.current === worker) workerRef.current = null;
      setStats(null);
    };
  }, [canvasKey, upscaling, setGivenUp, setResolution, setStats, setUpscaleStopped]);

  useEffect(() => {
    workerRef.current?.postMessage({
      type: 'view',
      width: settled.width,
      height: settled.height,
      pixelRatio
    } satisfies ToVideoWorker);
  }, [settled.width, settled.height, pixelRatio]);

  useEffect(() => {
    if (upscale !== 'off') {
      workerRef.current?.postMessage({
        type: 'sharpness',
        sharpness: upscale
      } satisfies ToVideoWorker);
    }
  }, [upscale]);

  return (
    <div className="relative h-full min-h-0 w-full min-w-0 overflow-hidden">
      <div
        ref={containerRef}
        data-kvm-screen-scroll
        className="flex h-full min-h-0 w-full min-w-0 items-start justify-start overflow-auto overscroll-contain bg-black"
      >
        <canvas
          key={`${canvasKey}:${upscaling ? 'upscaled' : 'plain'}`}
          id="screen"
          ref={screenRef}
          data-video-transport="quic"
          {...pictureAttributes(resolution)}
          className={clsx('m-auto block select-none', mouseStyle)}
          style={mediaStyle}
        ></canvas>
      </div>

      {video.state !== 'playing' && (
        <div className="pointer-events-none absolute inset-0 z-50 flex items-center justify-center bg-black/30 backdrop-blur-[2px] transition-all duration-300">
          <Spin size="large" />
        </div>
      )}
    </div>
  );
};
