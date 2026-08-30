import type { CSSProperties, RefObject } from 'react';
import { useEffect, useMemo, useRef, useState } from 'react';

import type { Resolution } from '@/types';

type ScreenFit = {
  containerRef: RefObject<HTMLDivElement>;
  mediaStyle: CSSProperties;
};

const FALLBACK_SCREEN_WIDTH = 1920;
const FALLBACK_SCREEN_HEIGHT = 1080;

export function useScreenFit(
  resolution: Resolution | null,
  videoScale: number,
  fallbackStyle: CSSProperties
): ScreenFit {
  const containerRef = useRef<HTMLDivElement>(null);
  const [fitScale, setFitScale] = useState(1);

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

      const nextScale = Math.min(rect.width / width, rect.height / height, 1);
      setFitScale(Math.max(0.05, nextScale));
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
  }, [resolution?.height, resolution?.width]);

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

  const mediaStyle = useMemo<CSSProperties>(() => {
    const scale = fitScale * videoScale;
    const width =
      resolution?.width && resolution.width > 0 ? resolution.width : FALLBACK_SCREEN_WIDTH;
    const height =
      resolution?.height && resolution.height > 0 ? resolution.height : FALLBACK_SCREEN_HEIGHT;

    if (!resolution?.width || !resolution?.height) {
      return {
        ...fallbackStyle,
        width: Math.max(1, Math.round(width * scale)),
        height: Math.max(1, Math.round(height * scale)),
        maxWidth: undefined,
        maxHeight: undefined,
        objectFit: 'cover'
      };
    }

    return {
      width: Math.max(1, Math.round(resolution.width * scale)),
      height: Math.max(1, Math.round(resolution.height * scale)),
      objectFit: 'cover'
    };
  }, [fallbackStyle, fitScale, resolution?.height, resolution?.width, videoScale]);

  return { containerRef, mediaStyle };
}
