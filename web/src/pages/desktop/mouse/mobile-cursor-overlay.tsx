import { useEffect, useRef, useState } from 'react';
import { MousePointer2Icon } from 'lucide-react';

import { MOBILE_CURSOR_EVENT, type MobileCursorEventDetail } from './mobile-cursor-events.ts';

type CursorState = {
  x: number;
  y: number;
  visible: boolean;
};

const HIDE_DELAY_MS = 900;

export const MobileCursorOverlay = () => {
  const [cursor, setCursor] = useState<CursorState>({ x: 0, y: 0, visible: false });
  const hideTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const cursorRatioRef = useRef({ x: 0.5, y: 0.5 });
  const visibleRef = useRef(false);

  useEffect(() => {
    let observedScreen: Element | null = null;
    const resizeObserver =
      typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(syncVisiblePosition);
    const mutationObserver =
      typeof MutationObserver === 'undefined'
        ? null
        : new MutationObserver(() => {
            observeScreen();
            syncVisiblePosition();
          });

    window.addEventListener(MOBILE_CURSOR_EVENT, handleCursorEvent);
    window.addEventListener('resize', syncVisiblePosition);
    window.addEventListener('orientationchange', syncVisiblePosition);
    window.addEventListener('scroll', syncVisiblePosition, true);
    observeScreen();

    const pointerSurface = document.getElementById('kvm-pointer-surface');
    if (pointerSurface && mutationObserver) {
      mutationObserver.observe(pointerSurface, { childList: true, subtree: true });
    }

    function handleCursorEvent(event: Event) {
      const detail = (event as CustomEvent<MobileCursorEventDetail>).detail;
      if (!detail) return;

      switch (detail.type) {
        case 'absolute':
          cursorRatioRef.current = {
            x: clamp01(detail.xRatio),
            y: clamp01(detail.yRatio)
          };
          showAtRatio();
          break;
        case 'relative':
          cursorRatioRef.current = {
            x: clamp01(cursorRatioRef.current.x + detail.deltaXRatio),
            y: clamp01(cursorRatioRef.current.y + detail.deltaYRatio)
          };
          showAtRatio();
          break;
        case 'show':
          showAtRatio();
          break;
        case 'hide':
          scheduleHide(detail.delay);
          break;
        default:
          break;
      }
    }

    function showAtRatio() {
      clearHideTimer();
      const point = getScreenPoint(cursorRatioRef.current.x, cursorRatioRef.current.y);
      if (!point) {
        hideNow();
        return;
      }

      visibleRef.current = true;
      setCursor({
        x: point.x,
        y: point.y,
        visible: true
      });
    }

    function syncVisiblePosition() {
      if (!visibleRef.current) return;
      showAtRatio();
    }

    function observeScreen() {
      if (!resizeObserver) return;

      const screen = document.getElementById('screen');
      if (!screen || screen === observedScreen) return;

      if (observedScreen) {
        resizeObserver.unobserve(observedScreen);
      }

      observedScreen = screen;
      resizeObserver.observe(screen);
    }

    function scheduleHide(delay = HIDE_DELAY_MS) {
      clearHideTimer();
      hideTimerRef.current = setTimeout(() => {
        hideTimerRef.current = null;
        hideNow();
      }, delay);
    }

    function hideNow() {
      visibleRef.current = false;
      setCursor((current) => ({ ...current, visible: false }));
    }

    function clearHideTimer() {
      if (hideTimerRef.current === null) return;
      clearTimeout(hideTimerRef.current);
      hideTimerRef.current = null;
    }

    return () => {
      clearHideTimer();
      resizeObserver?.disconnect();
      mutationObserver?.disconnect();
      window.removeEventListener(MOBILE_CURSOR_EVENT, handleCursorEvent);
      window.removeEventListener('resize', syncVisiblePosition);
      window.removeEventListener('orientationchange', syncVisiblePosition);
      window.removeEventListener('scroll', syncVisiblePosition, true);
    };
  }, []);

  return (
    <div
      aria-hidden="true"
      className="pointer-events-none fixed z-[900] transition-opacity duration-100"
      style={{
        left: cursor.x,
        top: cursor.y,
        opacity: cursor.visible ? 1 : 0,
        transform: 'translate3d(0, 0, 0)'
      }}
    >
      <MousePointer2Icon
        size={22}
        strokeWidth={2.25}
        className="text-white [filter:drop-shadow(0_1px_2px_rgba(0,0,0,0.95))]"
      />
    </div>
  );
};

function getScreenPoint(xRatio: number, yRatio: number): { x: number; y: number } | null {
  const screen = document.getElementById('screen');
  if (!screen) return null;

  const rect = screen.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return null;

  return {
    x: rect.left + rect.width * clamp01(xRatio),
    y: rect.top + rect.height * clamp01(yRatio)
  };
}

function clamp01(value: number): number {
  return Math.max(0, Math.min(1, value));
}
