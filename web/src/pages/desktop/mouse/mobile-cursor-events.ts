import type { Resolution } from '@/types';

export const MOBILE_CURSOR_EVENT = 'hardened-blikvm:mobile-cursor';

export type MobileCursorEventDetail =
  | {
      type: 'absolute';
      xRatio: number;
      yRatio: number;
    }
  | {
      type: 'relative';
      deltaXRatio: number;
      deltaYRatio: number;
    }
  | {
      type: 'show';
    }
  | {
      type: 'hide';
      delay?: number;
    };

export function emitMobileCursorAbsolute(xRatio: number, yRatio: number) {
  emit({
    type: 'absolute',
    xRatio: clamp01(xRatio),
    yRatio: clamp01(yRatio)
  });
}

export function emitMobileCursorRelative(
  deltaX: number,
  deltaY: number,
  resolution: Resolution | null
) {
  if (deltaX === 0 && deltaY === 0) return;

  const screen = document.getElementById('screen');
  const rect = screen?.getBoundingClientRect();
  const width = resolution?.width && resolution.width > 0 ? resolution.width : rect?.width;
  const height = resolution?.height && resolution.height > 0 ? resolution.height : rect?.height;

  if (!width || !height || width <= 0 || height <= 0) return;

  emit({
    type: 'relative',
    deltaXRatio: deltaX / width,
    deltaYRatio: deltaY / height
  });
}

export function emitMobileCursorShow() {
  emit({ type: 'show' });
}

export function emitMobileCursorHide(delay?: number) {
  emit({ type: 'hide', delay });
}

function emit(detail: MobileCursorEventDetail) {
  if (typeof window === 'undefined') return;
  window.dispatchEvent(new CustomEvent<MobileCursorEventDetail>(MOBILE_CURSOR_EVENT, { detail }));
}

function clamp01(value: number): number {
  return Math.max(0, Math.min(1, value));
}
