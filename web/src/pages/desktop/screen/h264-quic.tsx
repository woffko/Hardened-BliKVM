import { useEffect, useRef, useState } from 'react';
import { Spin } from 'antd';
import clsx from 'clsx';
import { useAtom, useAtomValue, useSetAtom } from 'jotai';

import * as storage from '@/lib/localstorage.ts';
import { quicSession } from '@/lib/quic/session.ts';
import type { FromVideoWorker, ToVideoWorker } from '@/lib/quic/video.worker.ts';
import VideoWorker from '@/lib/quic/video.worker.ts?worker';
import { mouseStyleAtom } from '@/jotai/mouse.ts';
import { resolutionAtom, videoScaleAtom } from '@/jotai/screen.ts';
import { quicVideoAtom, quicVideoGivenUpAtom, quicVideoStatsAtom } from '@/jotai/transport.ts';

import { useScreenFit } from './use-screen-fit.ts';

/**
 * A picture that shows no new frame for this long is not coming: even a
 * still screen gives one every two seconds. H.264 then goes on over WebRTC.
 */
const STALL_MS = 10000;

/** The bytes of `bytes` as a buffer of their own, to hand to the worker. */
function buffer(bytes: Uint8Array): ArrayBuffer {
  const whole = bytes.byteOffset === 0 && bytes.byteLength === bytes.buffer.byteLength;
  return (whole ? bytes.buffer : bytes.slice().buffer) as ArrayBuffer;
}

// H.264 over the QUIC session: the session delivers datagrams and key-frame
// streams, a worker puts the frames together, decodes them with WebCodecs
// and draws them on this canvas.
export const H264Quic = () => {
  const [resolution, setResolution] = useAtom(resolutionAtom);
  const mouseStyle = useAtomValue(mouseStyleAtom);
  const [videoScale, setVideoScale] = useAtom(videoScaleAtom);
  const video = useAtomValue(quicVideoAtom);
  const setStats = useSetAtom(quicVideoStatsAtom);
  const setGivenUp = useSetAtom(quicVideoGivenUpAtom);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  // A canvas can be handed to a worker once; a new one is mounted for the
  // next worker.
  const [canvasKey, setCanvasKey] = useState(0);
  const { containerRef, mediaStyle } = useScreenFit(resolution, videoScale, {
    maxWidth: '100%',
    maxHeight: '100%',
    objectFit: 'scale-down'
  });

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
    const post = (message: ToVideoWorker, transfer: Transferable[] = []) =>
      worker.postMessage(message, transfer);
    post({ type: 'canvas', canvas: offscreen }, [offscreen]);
    // The frames drawn at the last report, and when one was last drawn.
    let frames = 0;
    let progressAt = performance.now();
    worker.onmessage = (event: MessageEvent<FromVideoWorker>) => {
      const message = event.data;
      switch (message.type) {
        case 'request':
          quicSession.requestKeyFrame(message.stream, message.lastFrame, message.dropped);
          break;
        case 'picture':
          // The decoded frame's size is the target's real resolution.
          setResolution({ width: message.width, height: message.height });
          storage.setResolution({ width: message.width, height: message.height });
          break;
        case 'stats':
          if (message.frames !== frames || !quicSession.isOpen()) {
            // A session that is lost is the session's to report.
            frames = message.frames;
            progressAt = performance.now();
          } else if (performance.now() - progressAt > STALL_MS) {
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
            queued: message.queued
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
          break;
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
      quicSession.unwatch();
      worker.terminate();
      setStats(null);
    };
  }, [canvasKey, setGivenUp, setResolution, setStats]);

  return (
    <div className="relative h-full min-h-0 w-full min-w-0 overflow-hidden">
      <div
        ref={containerRef}
        data-kvm-screen-scroll
        className="flex h-full min-h-0 w-full min-w-0 items-start justify-start overflow-auto overscroll-contain bg-black"
      >
        <canvas
          key={canvasKey}
          id="screen"
          ref={canvasRef}
          data-video-transport="quic"
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
