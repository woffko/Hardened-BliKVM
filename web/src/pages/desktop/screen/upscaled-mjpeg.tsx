import type { CSSProperties } from 'react';
import { useEffect, useRef } from 'react';
import clsx from 'clsx';

import type { Resolution } from '@/types';
import { boundaryOf, MjpegParts } from '@/lib/mjpeg-parts.ts';
import { reportVideoUpscale } from '@/lib/page-video.ts';
import { pictureAttributes } from '@/lib/pointer-map.ts';
import { drawing, type Sharpness } from '@/lib/upscale/settings.ts';
import { Upscaler, type UpscaleReport } from '@/lib/upscale/upscaler.ts';
import { UpscaleWatchdog, type UpscaleStop } from '@/lib/upscale/watchdog.ts';
import { useScreenRef } from '@/hooks/useScreenRef.ts';

type Props = {
  /** The stream's URL; none while the view waits to ask again. */
  url: string | undefined;
  sharpness: Sharpness;
  mediaStyle: CSSProperties;
  drawn: { width: number; height: number };
  pixelRatio: number;
  resolution: Resolution | null;
  mouseStyle: string;
  /** A frame of this size was drawn. */
  onPicture: (width: number, height: number) => void;
  /** The stream broke or ended: the view asks for it again, as for its image. */
  onError: () => void;
  /** Upscaling cannot go on: no WebGL2 here, its context lost, or frames lost to it. */
  onStopped: (reason: UpscaleStop) => void;
};

const REPORT_MS = 1000;
const WINDOW_MS = 5000;
/**
 * For the watchdog, frames are counted as received at most one in this
 * time by their capture (`X-Timestamp`): a stream faster than the display
 * shows fewer of its frames on an image too. The page's own clock will not
 * do: a draw that holds up the page's thread holds up the reading of the
 * stream as well, and its frames then come together.
 */
const DUE_MS = 20;

const median = (values: number[]) => {
  if (!values.length) return null;
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.floor(sorted.length / 2)];
};

/**
 * The MJPEG view's picture when upscaled (docs/design/screen-upscaling-v1.md):
 * the stream is read with `fetch` and cut into its JPEG frames, each frame
 * decoded off the page's thread (`createImageBitmap`) and the newest drawn
 * on the next animation frame through the upscaler. A frame is uploaded
 * once, not on every animation frame as an `<img>` would have to be; a
 * hidden page decodes nothing. The watchdog counts the frames drawn against
 * those that came (`DUE_MS`).
 */
export const UpscaledMjpeg = ({
  url,
  sharpness,
  mediaStyle,
  drawn,
  pixelRatio,
  resolution,
  mouseStyle,
  onPicture,
  onError,
  onStopped
}: Props) => {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const screenRef = useScreenRef(canvasRef);
  const viewRef = useRef({ ...drawn, pixelRatio, sharpness });
  viewRef.current = { ...drawn, pixelRatio, sharpness };
  const callbacks = useRef({ onPicture, onError, onStopped });
  callbacks.current = { onPicture, onError, onStopped };
  // Set when the picture is shown at another size or sharpness: the last
  // frame is drawn again, not left stretched until the next one comes.
  const redrawRef = useRef(false);
  useEffect(() => {
    redrawRef.current = true;
  }, [drawn.width, drawn.height, pixelRatio, sharpness]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!url || !canvas) return;
    const upscaler = Upscaler.on(canvas, { onLost: () => callbacks.current.onStopped('lost') });
    if (!upscaler) {
      callbacks.current.onStopped('unavailable');
      return;
    }
    const controller = new AbortController();
    let ended = false;
    let animation = 0;
    // The newest decoded frame waiting to be drawn, and a frame waiting for
    // the decoder while it is busy: older ones are passed over.
    let ready: ImageBitmap | null = null;
    let waiting: Uint8Array | null = null;
    let decoding = false;
    let mode: UpscaleReport['mode'] = 'plain';
    let width = 0;
    let height = 0;
    let draws: { at: number; ms: number }[] = [];
    let gpu: { at: number; ms: number }[] = [];
    const counts = { received: 0, decoded: 0, drawn: 0 };
    const watchdog = new UpscaleWatchdog(performance.now());
    let watchdogTold = false;
    // The capture time of the last frame counted as received.
    let countedMs: number | null = null;
    // A hidden page decodes nothing; the count starts over when it is seen.
    const seen = () => {
      if (document.hidden) return;
      watchdog.reset(performance.now());
      countedMs = null;
    };
    document.addEventListener('visibilitychange', seen);

    const decode = async (jpeg: Uint8Array) => {
      if (decoding) {
        waiting = jpeg;
        return;
      }
      decoding = true;
      let next: Uint8Array | null = jpeg;
      try {
        while (next && !ended) {
          waiting = null;
          const bitmap = await createImageBitmap(
            new Blob([next as BlobPart], { type: 'image/jpeg' })
          );
          if (ended) {
            bitmap.close();
            break;
          }
          counts.decoded += 1;
          ready?.close();
          ready = bitmap;
          next = waiting;
        }
      } catch {
        if (!ended) callbacks.current.onError();
      } finally {
        decoding = false;
      }
    };

    // The frame drawn last, kept to draw again at another size.
    let shown: ImageBitmap | null = null;
    const frame = () => {
      if (ended) return;
      let bitmap = ready;
      ready = null;
      // A frame not drawn before, not one drawn again at another size.
      const fresh = bitmap !== null;
      if (bitmap) {
        shown?.close();
        shown = bitmap;
      } else if (redrawRef.current && shown) {
        bitmap = shown;
      }
      redrawRef.current = false;
      if (bitmap) {
        const view = viewRef.current;
        const how = drawing(
          bitmap.width,
          bitmap.height,
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
        if (upscaler.draw(bitmap, bitmap.width, bitmap.height, { mode: how.mode })) {
          const now = performance.now();
          draws.push({ at: now, ms: now - started });
          for (const ms of upscaler.takeGpuTimes()) gpu.push({ at: now, ms });
          if (mode !== how.mode || canvas.dataset.upscale !== how.mode) {
            canvas.dataset.upscale = how.mode;
          }
          mode = how.mode;
          counts.drawn += 1;
          if (fresh) watchdog.draw(now);
          canvas.dataset.upscaleBuffer = `${canvas.width}x${canvas.height}`;
          canvas.dataset.mjpegFrames = `${counts.received}/${counts.decoded}/${counts.drawn}`;
        }
        if (bitmap.width !== width || bitmap.height !== height) {
          width = bitmap.width;
          height = bitmap.height;
          callbacks.current.onPicture(width, height);
        }
      }
      animation = window.requestAnimationFrame(frame);
    };
    animation = window.requestAnimationFrame(frame);

    const reporter = window.setInterval(() => {
      const now = performance.now();
      if (!watchdogTold && watchdog.losing(now)) {
        watchdogTold = true;
        callbacks.current.onStopped('slow');
      }
      draws = draws.filter((entry) => now - entry.at <= WINDOW_MS);
      gpu = gpu.filter((entry) => now - entry.at <= WINDOW_MS);
      const frames = watchdog.rates(now);
      reportVideoUpscale({
        mode,
        drawMs: median(draws.map((entry) => entry.ms)),
        gpuMs: median(gpu.map((entry) => entry.ms)),
        frames
      });
      canvas.dataset.upscaleFrames = `${frames.drawn.toFixed(1)}/${frames.received.toFixed(1)}`;
      // What a probe reads without opening a menu.
      canvas.dataset.upscale = mode;
      canvas.dataset.upscaleBuffer = `${canvas.width}x${canvas.height}`;
      canvas.dataset.mjpegFrames = `${counts.received}/${counts.decoded}/${counts.drawn}`;
    }, REPORT_MS);

    void (async () => {
      try {
        const response = await fetch(url, {
          credentials: 'same-origin',
          cache: 'no-store',
          signal: controller.signal
        });
        const boundary = boundaryOf(response.headers.get('content-type'));
        if (!response.ok || !response.body || !boundary) throw new Error('no stream');
        const parts = new MjpegParts(boundary);
        const reader = response.body.getReader();
        for (;;) {
          const { value, done } = await reader.read();
          if (done || ended) break;
          for (const part of parts.push(value)) {
            counts.received += 1;
            // A hidden page decodes nothing; the next frame is shown when it is back.
            if (document.hidden) continue;
            const captured = part.capturedMs;
            if (captured === null || countedMs === null || captured - countedMs >= DUE_MS) {
              countedMs = captured;
              watchdog.receive(performance.now());
            }
            void decode(part.jpeg);
          }
        }
        // The stream ended: its last frame is drawn before it is asked for again.
        for (let wait = 0; wait < 60 && !ended && (decoding || ready); wait += 1) {
          await new Promise((resolve) => window.setTimeout(resolve, 16));
        }
        if (!ended) callbacks.current.onError();
      } catch {
        if (!ended) callbacks.current.onError();
      }
    })();

    return () => {
      ended = true;
      document.removeEventListener('visibilitychange', seen);
      controller.abort();
      window.cancelAnimationFrame(animation);
      window.clearInterval(reporter);
      ready?.close();
      shown?.close();
      reportVideoUpscale(null);
      upscaler.dispose();
    };
  }, [url]);

  return (
    <canvas
      id="screen"
      ref={screenRef}
      {...pictureAttributes(resolution)}
      className={clsx('m-auto block select-none', mouseStyle)}
      style={mediaStyle}
    />
  );
};
