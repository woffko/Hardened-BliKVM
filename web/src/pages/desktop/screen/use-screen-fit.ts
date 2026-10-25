import type { CSSProperties, RefObject } from 'react';
import { useEffect, useMemo, useRef, useState } from 'react';

import type { Resolution } from '@/types';
import { fitScale as scaleToFit, type Fit } from '@/lib/upscale/settings.ts';
import { usePixelRatio } from '@/hooks/usePixelRatio.ts';

type ScreenFit = {
  containerRef: RefObject<HTMLDivElement>;
  mediaStyle: CSSProperties;
  /** The picture's size on the page, in CSS pixels. */
  drawn: { width: number; height: number };
  /**
   * The same once it has stopped changing for a moment: what a canvas
   * buffer follows, so that a pinch is stretched by the browser while it
   * goes on and drawn again at its final size.
   */
  settled: { width: number; height: number };
  /** Device pixels per CSS pixel. */
  pixelRatio: number;
  /**
   * How much the picture is enlarged on the display: the fit, times the
   * zoom, times the pixel ratio; above 1 the browser enlarges it.
   */
  enlargement: number;
};

/**
 * The screen element's classes: centred in the view while it fits, and laid
 * out at the size it is drawn at from the start. A flex item would shrink to
 * the view while its picture has no size of its own (an image or a video
 * before its first frame, a canvas before its first buffer): the view's
 * centring then finds nothing to scroll, and the picture, once it comes, is
 * shown from its left edge.
 */
export const SCREEN_CLASS = 'm-auto block shrink-0 select-none';

const FALLBACK_SCREEN_WIDTH = 1920;
const FALLBACK_SCREEN_HEIGHT = 1080;
/** How long the drawn size must stay before a buffer follows it. */
const SETTLE_MS = 150;

export function useScreenFit(
  resolution: Resolution | null,
  videoScale: number,
  fallbackStyle: CSSProperties,
  fit: Fit = 'fit'
): ScreenFit {
  const containerRef = useRef<HTMLDivElement>(null);
  const [fitScale, setFitScale] = useState(1);
  const pixelRatio = usePixelRatio();
  const fitKey = typeof fit === 'string' ? fit : `${fit.target.width}x${fit.target.height}`;
  const fitRef = useRef(fit);
  fitRef.current = fit;

  useEffect(() => {
    function updateFitScale() {
      if (!containerRef.current) {
        return;
      }

      const width =
        resolution?.width && resolution.width > 0 ? resolution.width : FALLBACK_SCREEN_WIDTH;
      const height =
        resolution?.height && resolution.height > 0 ? resolution.height : FALLBACK_SCREEN_HEIGHT;
      const rect = containerRef.current.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) {
        return;
      }

      setFitScale(scaleToFit(rect, { width, height }, fitRef.current));
    }

    updateFitScale();

    const container = containerRef.current;
    if (typeof ResizeObserver === 'undefined' || !container) {
      window.addEventListener('resize', updateFitScale);
      window.addEventListener('orientationchange', updateFitScale);
      return () => {
        window.removeEventListener('resize', updateFitScale);
        window.removeEventListener('orientationchange', updateFitScale);
      };
    }

    const observer = new ResizeObserver(updateFitScale);
    observer.observe(container);
    window.addEventListener('orientationchange', updateFitScale);

    return () => {
      observer.disconnect();
      window.removeEventListener('orientationchange', updateFitScale);
    };
  }, [fitKey, resolution?.height, resolution?.width]);

  useEffect(() => {
    if (!containerRef.current) return;

    const frame = window.requestAnimationFrame(() => {
      const container = containerRef.current;
      if (!container) return;

      container.scrollLeft = Math.max(0, (container.scrollWidth - container.clientWidth) / 2);
      container.scrollTop = Math.max(0, (container.scrollHeight - container.clientHeight) / 2);
    });

    return () => window.cancelAnimationFrame(frame);
  }, [fitScale, resolution?.height, resolution?.width, videoScale]);

  const drawn = useMemo(() => {
    const scale = fitScale * videoScale;
    const width =
      resolution?.width && resolution.width > 0 ? resolution.width : FALLBACK_SCREEN_WIDTH;
    const height =
      resolution?.height && resolution.height > 0 ? resolution.height : FALLBACK_SCREEN_HEIGHT;
    return {
      width: Math.max(1, Math.round(width * scale)),
      height: Math.max(1, Math.round(height * scale))
    };
  }, [fitScale, resolution?.height, resolution?.width, videoScale]);

  const mediaStyle = useMemo<CSSProperties>(() => {
    if (!resolution?.width || !resolution?.height) {
      return {
        ...fallbackStyle,
        ...drawn,
        maxWidth: undefined,
        maxHeight: undefined,
        objectFit: 'cover'
      };
    }

    return { ...drawn, objectFit: 'cover' };
  }, [drawn, fallbackStyle, resolution?.height, resolution?.width]);

  const enlargement = fitScale * videoScale * pixelRatio;

  const [settled, setSettled] = useState(drawn);
  useEffect(() => {
    if (settled.width === drawn.width && settled.height === drawn.height) return;
    const timer = window.setTimeout(() => setSettled(drawn), SETTLE_MS);
    return () => window.clearTimeout(timer);
  }, [drawn, settled]);

  // What a probe reads without opening a menu.
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    container.dataset.pictureEnlargement = enlargement.toFixed(3);
    container.dataset.picturePixelRatio = String(pixelRatio);
  }, [enlargement, pixelRatio]);

  return { containerRef, mediaStyle, drawn, settled, pixelRatio, enlargement };
}
