import type { CSSProperties, MutableRefObject, SyntheticEvent } from 'react';
import { useEffect, useRef } from 'react';
import clsx from 'clsx';

import type { Resolution } from '@/types';
import { counted } from '@/lib/busy.ts';
import { reportVideoUpscale } from '@/lib/page-video.ts';
import { pictureAttributes } from '@/lib/pointer-map.ts';
import { drawing, type Sharpness } from '@/lib/upscale/settings.ts';
import { Upscaler, type UpscaleReport } from '@/lib/upscale/upscaler.ts';
import { UpscaleWatchdog, type UpscaleStop } from '@/lib/upscale/watchdog.ts';
import { useScreenRef } from '@/hooks/useScreenRef.ts';

import { SCREEN_CLASS } from './use-screen-fit.ts';

type Props = {
  videoRef: MutableRefObject<HTMLVideoElement | null>;
  /** The sharpness to enlarge with; `off` draws the video itself as the screen. */
  upscale: Sharpness | 'off';
  /** The board's VP8 picture: drawn through VP8's range table. */
  vp8Range: boolean;
  /** The plain video's style: the screen fit's, with VP8's SVG filter where it applies. */
  videoStyle: CSSProperties;
  mediaStyle: CSSProperties;
  drawn: { width: number; height: number };
  pixelRatio: number;
  resolution: Resolution | null;
  mouseStyle: string;
  onResize?: (event: SyntheticEvent<HTMLVideoElement>) => void;
  onPlaying?: (event: SyntheticEvent<HTMLVideoElement>) => void;
  /** Upscaling cannot go on: no WebGL2 here, its context lost, or frames lost to it. */
  onStopped: (reason: UpscaleStop) => void;
};

/** How often what the drawing measures is handed to the quality monitor. */
const REPORT_MS = 1000;
/** What a report says about the frames covers this much of the past. */
const WINDOW_MS = 5000;

const median = (values: number[]) => {
  if (!values.length) return null;
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.floor(sorted.length / 2)];
};

/**
 * The WebRTC views' picture (docs/design/screen-upscaling-v1.md). Plainly,
 * the video element is the screen. Upscaled, the video plays out of sight
 * and a canvas is the screen: each frame the video presents
 * (`requestVideoFrameCallback`, else every animation frame) is drawn through
 * the upscaler into a buffer of the size the picture is shown at. The
 * watchdog compares the frames the video presented with those drawn: a draw
 * that holds the page's thread up lets the video present frames it never
 * hears of.
 */
export const UpscaledVideo = ({
  videoRef,
  upscale,
  vp8Range,
  videoStyle,
  mediaStyle,
  drawn,
  pixelRatio,
  resolution,
  mouseStyle,
  onResize,
  onPlaying,
  onStopped
}: Props) => {
  const upscaling = upscale !== 'off';
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  // The screen is the canvas when upscaling, else the video.
  const canvasScreenRef = useScreenRef(canvasRef);
  const videoScreenRef = useScreenRef(videoRef);
  const viewRef = useRef({ ...drawn, pixelRatio, sharpness: upscale });
  viewRef.current = { ...drawn, pixelRatio, sharpness: upscale };
  const stoppedRef = useRef(onStopped);
  stoppedRef.current = onStopped;
  // Draws the video's current frame again, as when the picture is shown at
  // another size: set by the drawing loop.
  const redrawRef = useRef<(() => void) | null>(null);
  useEffect(() => {
    redrawRef.current?.();
  }, [drawn.width, drawn.height, pixelRatio, upscale]);

  useEffect(() => {
    const video = videoRef.current;
    const canvas = canvasRef.current;
    if (!upscaling || !video || !canvas) return;
    const upscaler = Upscaler.on(canvas, { onLost: () => stoppedRef.current('lost') });
    if (!upscaler) {
      stoppedRef.current('unavailable');
      return;
    }
    let ended = false;
    let handle = 0;
    let mode: UpscaleReport['mode'] = 'plain';
    let draws: { at: number; ms: number }[] = [];
    let gpu: { at: number; ms: number }[] = [];
    const frameCallbacks = 'requestVideoFrameCallback' in video;
    // Without frame callbacks the frames the video presented are not known.
    const watchdog = frameCallbacks ? new UpscaleWatchdog(performance.now()) : null;
    let watchdogTold = false;
    let presentedBefore: number | null = null;
    // A hidden page is shown no frames; the count starts over when it is seen.
    const seen = () => {
      if (document.hidden) return;
      watchdog?.reset(performance.now());
      presentedBefore = null;
    };
    document.addEventListener('visibilitychange', seen);
    /** Draws the video's current frame; whether it was drawn. */
    const paint = (): boolean => {
      const width = video.videoWidth;
      const height = video.videoHeight;
      const view = viewRef.current;
      if (width > 0 && height > 0 && view.sharpness !== 'off') {
        const how = drawing(
          width,
          height,
          view.width,
          view.height,
          view.pixelRatio,
          view.sharpness
        );
        if (canvas.width !== how.width || canvas.height !== how.height) {
          canvas.width = how.width;
          canvas.height = how.height;
        }
        const started = performance.now();
        if (upscaler.draw(video, width, height, { mode: how.mode, vp8Range })) {
          const now = performance.now();
          draws.push({ at: now, ms: now - started });
          for (const ms of upscaler.takeGpuTimes()) gpu.push({ at: now, ms });
          mode = how.mode;
          return true;
        }
      }
      return false;
    };
    const draw = (_now?: number, metadata?: VideoFrameCallbackMetadata) =>
      counted(() => {
        if (ended) return;
        drawFrame(metadata);
      });
    const drawFrame = (metadata?: VideoFrameCallbackMetadata) => {
      const drew = paint();
      if (watchdog && metadata) {
        const at = performance.now();
        const came =
          presentedBefore === null ? 1 : Math.max(1, metadata.presentedFrames - presentedBefore);
        presentedBefore = metadata.presentedFrames;
        for (let frame = 0; frame < came; frame += 1) watchdog.receive(at);
        if (drew) watchdog.draw(at);
      }
      handle = frameCallbacks
        ? video.requestVideoFrameCallback(draw)
        : window.requestAnimationFrame(draw);
    };
    redrawRef.current = () => {
      if (!ended) window.requestAnimationFrame(() => void (!ended && paint()));
    };
    handle = frameCallbacks
      ? video.requestVideoFrameCallback(draw)
      : window.requestAnimationFrame(draw);
    const reporter = window.setInterval(() => {
      const now = performance.now();
      if (watchdog && !watchdogTold && watchdog.losing(now)) {
        watchdogTold = true;
        stoppedRef.current('slow');
      }
      draws = draws.filter((entry) => now - entry.at <= WINDOW_MS);
      gpu = gpu.filter((entry) => now - entry.at <= WINDOW_MS);
      const report: UpscaleReport = {
        mode,
        drawMs: median(draws.map((entry) => entry.ms)),
        gpuMs: median(gpu.map((entry) => entry.ms)),
        frames: watchdog ? watchdog.rates(now) : null
      };
      reportVideoUpscale(report);
      // What a probe reads without opening a menu.
      canvas.dataset.upscale = report.mode;
      canvas.dataset.upscaleBuffer = `${canvas.width}x${canvas.height}`;
      canvas.dataset.upscaleFrames = report.frames
        ? `${report.frames.drawn.toFixed(1)}/${report.frames.received.toFixed(1)}`
        : '';
    }, REPORT_MS);
    return () => {
      ended = true;
      redrawRef.current = null;
      document.removeEventListener('visibilitychange', seen);
      if (frameCallbacks) video.cancelVideoFrameCallback(handle);
      else window.cancelAnimationFrame(handle);
      window.clearInterval(reporter);
      reportVideoUpscale(null);
      upscaler.dispose();
    };
  }, [upscaling, vp8Range, videoRef]);

  const stopClick = (event: SyntheticEvent) => {
    event.stopPropagation();
    event.preventDefault();
  };

  return (
    <>
      <video
        id={upscaling ? undefined : 'screen'}
        ref={upscaling ? videoRef : videoScreenRef}
        {...(upscaling ? {} : pictureAttributes(resolution))}
        data-upscale-source={upscaling ? '' : undefined}
        className={upscaling ? undefined : clsx(SCREEN_CLASS, mouseStyle)}
        // Out of sight but drawn, so that the browser presents its frames.
        style={
          upscaling
            ? {
                position: 'absolute',
                left: 0,
                top: 0,
                width: 1,
                height: 1,
                opacity: 0,
                pointerEvents: 'none'
              }
            : videoStyle
        }
        muted
        autoPlay
        playsInline
        controls={false}
        onClick={stopClick}
        onResize={onResize}
        onPlaying={onPlaying}
      />
      {upscaling && (
        <canvas
          id="screen"
          ref={canvasScreenRef}
          {...pictureAttributes(resolution)}
          className={clsx(SCREEN_CLASS, mouseStyle)}
          style={mediaStyle}
          onClick={stopClick}
        />
      )}
    </>
  );
};
