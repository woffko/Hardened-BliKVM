import { useEffect, useRef } from 'react';
import clsx from 'clsx';
import { useAtom, useAtomValue } from 'jotai';
import { w3cwebsocket as W3cWebSocket } from 'websocket';

import * as storage from '@/lib/localstorage.ts';
import { directCounters } from '@/lib/page-video.ts';
import { pictureAttributes } from '@/lib/pointer-map.ts';
import { getBaseUrl } from '@/lib/service.ts';
import { mouseStyleAtom } from '@/jotai/mouse';
import { resolutionAtom, videoScaleAtom } from '@/jotai/screen.ts';
import { useScreenRef } from '@/hooks/useScreenRef.ts';

import DirectWorker from './direct.worker.ts?worker';
import { useScreenFit } from './use-screen-fit.ts';

export const H264Direct = () => {
  const resolution = useAtomValue(resolutionAtom);
  const mouseStyle = useAtomValue(mouseStyleAtom);
  const [videoScale, setVideoScale] = useAtom(videoScaleAtom);

  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const screenRef = useScreenRef(canvasRef);
  const workerRef = useRef<Worker | null>(null);
  const { containerRef, mediaStyle } = useScreenFit(resolution, videoScale, {
    maxWidth: '100%',
    maxHeight: '100%',
    objectFit: 'scale-down'
  });

  useEffect(() => {
    const scale = storage.getVideoScale();
    if (scale) {
      setVideoScale(scale);
    }
  }, [setVideoScale]);

  useEffect(() => {
    if (!window.VideoDecoder) {
      console.log('Error: WebCodecs API not supported.');
      return;
    }
    if (!canvasRef.current) {
      return;
    }

    const worker = new DirectWorker();
    workerRef.current = worker;

    const offscreen = canvasRef.current.transferControlToOffscreen();
    worker.postMessage({ type: 'h264', canvas: offscreen }, [offscreen]);

    const url = `${getBaseUrl('ws')}/api/stream/h264/direct`;
    const ws = new W3cWebSocket(url);
    ws.binaryType = 'arraybuffer';
    // What the quality monitor reads: the bytes this picture was sent.
    directCounters.bytes = 0;
    directCounters.messages = 0;

    ws.onmessage = (event) => {
      if (event.data instanceof ArrayBuffer) {
        directCounters.bytes += event.data.byteLength;
        directCounters.messages += 1;
      }
      try {
        worker.postMessage({ type: 'ws_message', data: event.data }, [event.data]);
      } catch (error) {
        console.error('Error processing WebSocket message:', error);
      }
    };

    ws.onerror = () => {
      worker.postMessage({ type: 'error' });
    };

    ws.onclose = () => {
      worker.postMessage({ type: 'close' });
    };

    return () => {
      if (ws.readyState === 1) {
        ws.close();
      }
      worker.terminate();
    };
  }, []);

  return (
    <div
      ref={containerRef}
      data-kvm-screen-scroll
      className="flex h-full min-h-0 w-full min-w-0 items-start justify-start overflow-auto overscroll-contain bg-black"
    >
      <canvas
        id="screen"
        ref={screenRef}
        {...pictureAttributes(resolution)}
        className={clsx('m-auto block select-none', mouseStyle)}
        style={mediaStyle}
      ></canvas>
    </div>
  );
};
