import { useEffect, useRef } from 'react';
import { useAtom, useAtomValue } from 'jotai';

import * as storage from '@/lib/localstorage.ts';
import { MouseReportAbsolute } from '@/lib/mouse.ts';
import { pictureSize, pointInPicture } from '@/lib/pointer-map.ts';
import { client, MessageEvent } from '@/lib/websocket.ts';
import { scrollDirectionAtom, scrollIntervalAtom } from '@/jotai/mouse.ts';
import { resolutionAtom, videoScaleAtom } from '@/jotai/screen.ts';
import { useHidLease } from '@/hooks/useHidLease.ts';
import { useIsTouchLayout } from '@/hooks/useResponsiveLayout.ts';

import { emitMobileCursorAbsolute, emitMobileCursorHide } from './mobile-cursor-events.ts';
import { MouseAbsoluteEvent } from './types.ts';

enum MouseButton {
  Left = 0,
  Middle = 1,
  Right = 2,
  Back = 3,
  Forward = 4
}

const HID_ABSOLUTE_MAX = 0x7fff;
const HID_ABSOLUTE_CENTER = Math.round(HID_ABSOLUTE_MAX / 2);
const MIN_TOUCH_ZOOM = 0.1;
const MAX_TOUCH_ZOOM = 4;
// Distance kept between the cursor and the edge of the view when the view
// shrinks over it.
const CURSOR_VIEW_MARGIN = 48;

export const Absolute = () => {
  const isTouchLayout = useIsTouchLayout();
  const resolution = useAtomValue(resolutionAtom);
  const [videoScale, setVideoScale] = useAtom(videoScaleAtom);
  const scrollDirection = useAtomValue(scrollDirectionAtom);
  const scrollInterval = useAtomValue(scrollIntervalAtom);
  const { restricted, reportHid } = useHidLease();

  const mouseRef = useRef(new MouseReportAbsolute());
  const lastPosRef = useRef({ x: HID_ABSOLUTE_CENTER, y: HID_ABSOLUTE_CENTER });
  const lastResolutionKeyRef = useRef<string | null>(null);
  const lastScrollTimeRef = useRef(0);
  const currentVideoScaleRef = useRef(videoScale);

  // For touch events
  const touchStartTimeRef = useRef(0);
  const lastTouchYRef = useRef(0);
  const pinchDistanceRef = useRef<number | null>(null);
  const pinchScaleRef = useRef(videoScale);
  const longPressTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const isLongPressRef = useRef(false);
  const hasMoveRef = useRef(false);
  const isDraggingRef = useRef(false);
  const pressedButtonRef = useRef<MouseButton | null>(null);
  const touchStartPosRef = useRef({ x: 0, y: 0 });
  const lastTouchPosRef = useRef({ x: 0, y: 0 });
  const handleMouseEventRef = useRef(handleMouseEvent);
  handleMouseEventRef.current = handleMouseEvent;

  const TAP_THRESHOLD = 8;
  const DRAG_THRESHOLD = 10;
  const VELOCITY_THRESHOLD = 0.3;

  useEffect(() => {
    currentVideoScaleRef.current = videoScale;
  }, [videoScale]);

  useEffect(() => {
    const resolutionKey = `${resolution?.width ?? 0}x${resolution?.height ?? 0}`;

    if (lastResolutionKeyRef.current === null) {
      lastResolutionKeyRef.current = resolutionKey;
      return;
    }

    if (lastResolutionKeyRef.current === resolutionKey) {
      return;
    }

    lastResolutionKeyRef.current = resolutionKey;

    if (!isTouchLayout) {
      return;
    }

    if (longPressTimerRef.current) {
      clearTimeout(longPressTimerRef.current);
      longPressTimerRef.current = null;
    }

    if (pressedButtonRef.current !== null) {
      handleMouseEventRef.current({ type: 'mouseup', button: pressedButtonRef.current });
    }

    isLongPressRef.current = false;
    hasMoveRef.current = false;
    isDraggingRef.current = false;
    pressedButtonRef.current = null;
    lastPosRef.current = { x: HID_ABSOLUTE_CENTER, y: HID_ABSOLUTE_CENTER };

    handleMouseEventRef.current({
      type: 'move',
      x: HID_ABSOLUTE_CENTER,
      y: HID_ABSOLUTE_CENTER
    });
    emitMobileCursorAbsolute(0.5, 0.5);
  }, [isTouchLayout, resolution?.height, resolution?.width]);

  useEffect(() => {
    const screen = document.getElementById('screen') as HTMLElement | null;
    if (!screen) return;
    const screenElement = screen;
    const pointerSurface = document.getElementById('kvm-pointer-surface') as HTMLElement | null;
    const scrollContainer = screenElement.closest('[data-kvm-screen-scroll]') as HTMLElement | null;
    const mouseTarget = screenElement;
    const touchTarget = pointerSurface ?? screen;
    const gestureElements = uniqueElements([touchTarget, screen, scrollContainer]);
    const previousGestureStyles = gestureElements.map((element) => ({
      element,
      touchAction: element.style.touchAction,
      overscrollBehavior: element.style.overscrollBehavior,
      userSelect: element.style.userSelect,
      webkitUserSelect: element.style.webkitUserSelect
    }));
    const touchOptions: AddEventListenerOptions = { passive: false };

    for (const element of gestureElements) {
      element.style.touchAction = 'none';
      element.style.overscrollBehavior = 'contain';
      element.style.userSelect = 'none';
      element.style.webkitUserSelect = 'none';
    }

    mouseTarget.addEventListener('mousedown', handleMouseDown);
    mouseTarget.addEventListener('mouseup', handleMouseUp);
    mouseTarget.addEventListener('mousemove', handleMouseMove);
    mouseTarget.addEventListener('wheel', handleWheel);
    mouseTarget.addEventListener('click', disableEvent);
    mouseTarget.addEventListener('contextmenu', disableEvent);
    touchTarget.addEventListener('touchstart', handleTouchStart, touchOptions);
    touchTarget.addEventListener('touchmove', handleTouchMove, touchOptions);
    touchTarget.addEventListener('touchend', handleTouchEnd, touchOptions);
    touchTarget.addEventListener('touchcancel', handleTouchCancel, touchOptions);

    // On a phone the room for the screen shrinks when a keyboard opens. A
    // cursor that was in sight stays in sight: the image scrolls under it,
    // as a page brings its focused field above the keyboard.
    let lastView: DOMRect | null = null;
    const viewObserver =
      isTouchLayout && scrollContainer && typeof ResizeObserver !== 'undefined'
        ? new ResizeObserver(keepCursorInView)
        : null;
    if (scrollContainer) viewObserver?.observe(scrollContainer);

    function keepCursorInView() {
      if (!scrollContainer) return;
      const view = scrollContainer.getBoundingClientRect();
      const previous = lastView;
      lastView = view;
      const rect = screenElement.getBoundingClientRect();
      if (!previous || view.width <= 0 || view.height <= 0 || rect.height <= 0) return;

      const cursorX = rect.left + (rect.width * lastPosRef.current.x) / HID_ABSOLUTE_MAX;
      const cursorY = rect.top + (rect.height * lastPosRef.current.y) / HID_ABSOLUTE_MAX;
      // The view's size is in fractions of a pixel and a scroll offset is
      // not: between two steps of a resize the cursor may lag by a pixel.
      const slack = 2;
      const wasInSight =
        cursorX >= previous.left - slack &&
        cursorX <= previous.right + slack &&
        cursorY >= previous.top - slack &&
        cursorY <= previous.bottom + slack;
      if (!wasInSight) return;

      const marginX = Math.min(CURSOR_VIEW_MARGIN, view.width / 4);
      const marginY = Math.min(CURSOR_VIEW_MARGIN, view.height / 4);
      if (cursorY > view.bottom - marginY) {
        scrollContainer.scrollTop += cursorY - (view.bottom - marginY);
      } else if (cursorY < view.top + marginY) {
        scrollContainer.scrollTop -= view.top + marginY - cursorY;
      }
      if (cursorX > view.right - marginX) {
        scrollContainer.scrollLeft += cursorX - (view.right - marginX);
      } else if (cursorX < view.left + marginX) {
        scrollContainer.scrollLeft -= view.left + marginX - cursorX;
      }
    }

    // Mouse down event
    function handleMouseDown(e: MouseEvent) {
      disableEvent(e);
      handleMouseEventRef.current({ type: 'mousedown', button: e.button });
    }

    // Mouse up event
    function handleMouseUp(e: MouseEvent) {
      disableEvent(e);
      handleMouseEventRef.current({ type: 'mouseup', button: e.button });
    }

    // Mouse move event
    function handleMouseMove(e: MouseEvent) {
      disableEvent(e);
      const { x, y } = getCoordinate(e);
      handleMouseEventRef.current({ type: 'move', x, y });
    }

    // Mouse wheel event
    function handleWheel(e: WheelEvent) {
      disableEvent(e);

      if (Math.floor(e.deltaY) === 0) {
        return;
      }

      const currentTime = Date.now();
      if (currentTime - lastScrollTimeRef.current < scrollInterval) {
        return;
      }

      const deltaY = (e.deltaY > 0 ? 1 : -1) * scrollDirection;
      handleMouseEventRef.current({ type: 'wheel', deltaY });
      lastScrollTimeRef.current = currentTime;
    }

    // Mouse touch start event
    function handleTouchStart(e: TouchEvent) {
      disableEvent(e);

      if (e.touches.length === 0) {
        return;
      }

      const touch = e.touches[0];

      // Reset states
      touchStartTimeRef.current = Date.now();
      lastTouchYRef.current = touch.clientY;
      isLongPressRef.current = false;
      hasMoveRef.current = false;
      isDraggingRef.current = false;
      pressedButtonRef.current = null;
      touchStartPosRef.current = { x: touch.clientX, y: touch.clientY };
      lastTouchPosRef.current = { x: touch.clientX, y: touch.clientY };

      if (longPressTimerRef.current) {
        clearTimeout(longPressTimerRef.current);
      }

      emitCurrentMobileCursorPosition();

      if (e.touches.length > 1) {
        hasMoveRef.current = true;
        isDraggingRef.current = false;
        startPinchZoom(e.touches);
        return;
      }

      // Start long press
      longPressTimerRef.current = setTimeout(() => {
        isLongPressRef.current = true;
        pressedButtonRef.current = MouseButton.Right;
        if (navigator.vibrate) {
          navigator.vibrate(50);
        }

        handleMouseEventRef.current({ type: 'mousedown', button: MouseButton.Right });
      }, 800);
    }

    // Mouse touch move event
    function handleTouchMove(e: TouchEvent) {
      disableEvent(e);

      if (e.touches.length === 0) {
        return;
      }
      const touch = e.touches[0];

      if (e.touches.length > 1) {
        emitMobileCursorHide(0);
        hasMoveRef.current = true;
        isDraggingRef.current = false;
        clearLongPress();
        updatePinchZoom(e.touches);
        return;
      }

      const deltaX = Math.abs(touch.clientX - touchStartPosRef.current.x);
      const deltaY = Math.abs(touch.clientY - touchStartPosRef.current.y);
      const distance = Math.sqrt(deltaX * deltaX + deltaY * deltaY);

      const timeDelta = Date.now() - touchStartTimeRef.current;
      const velocity = timeDelta > 0 ? distance / timeDelta : 0;

      const shouldStartDrag =
        distance > DRAG_THRESHOLD || (distance > TAP_THRESHOLD && velocity > VELOCITY_THRESHOLD);

      if (shouldStartDrag && !isDraggingRef.current && !isLongPressRef.current) {
        if (!hasMoveRef.current) {
          hasMoveRef.current = true;
        }

        if (longPressTimerRef.current) {
          clearTimeout(longPressTimerRef.current);
          longPressTimerRef.current = null;
        }

        isDraggingRef.current = true;
      }

      if (distance > TAP_THRESHOLD && !hasMoveRef.current) {
        hasMoveRef.current = true;
      }

      if (isDraggingRef.current || isLongPressRef.current) {
        const { x, y, xRatio, yRatio } = getRelativeTouchCoordinate(touch);
        emitMobileCursorAbsolute(xRatio, yRatio);
        handleMouseEventRef.current({ type: 'move', x, y });
      }
    }

    // Mouse touch end event
    function handleTouchEnd(e: TouchEvent) {
      disableEvent(e);

      if (pinchDistanceRef.current !== null) {
        clearPinchZoom();
        clearLongPress();

        if (e.touches.length > 0) {
          const touch = e.touches[0];
          hasMoveRef.current = true;
          isDraggingRef.current = false;
          touchStartTimeRef.current = Date.now();
          touchStartPosRef.current = { x: touch.clientX, y: touch.clientY };
          lastTouchPosRef.current = { x: touch.clientX, y: touch.clientY };
          lastTouchYRef.current = touch.clientY;
          emitCurrentMobileCursorPosition();
          return;
        }

        isLongPressRef.current = false;
        hasMoveRef.current = false;
        isDraggingRef.current = false;
        pressedButtonRef.current = null;
        emitMobileCursorHide();
        return;
      }

      if (longPressTimerRef.current) {
        clearTimeout(longPressTimerRef.current);
        longPressTimerRef.current = null;
      }

      if (!hasMoveRef.current && !isLongPressRef.current) {
        handleMouseEventRef.current({ type: 'mousedown', button: MouseButton.Left });
        setTimeout(() => {
          handleMouseEventRef.current({ type: 'mouseup', button: MouseButton.Left });
        }, 50);
      } else if (pressedButtonRef.current !== null) {
        handleMouseEventRef.current({ type: 'mouseup', button: pressedButtonRef.current! });
      }

      isLongPressRef.current = false;
      hasMoveRef.current = false;
      isDraggingRef.current = false;
      pressedButtonRef.current = null;
      emitMobileCursorHide();
    }

    // Mouse touch cancel event
    function handleTouchCancel(e: any) {
      disableEvent(e);

      clearPinchZoom();

      if (longPressTimerRef.current) {
        clearTimeout(longPressTimerRef.current);
        longPressTimerRef.current = null;
      }

      if (pressedButtonRef.current !== null) {
        handleMouseEventRef.current({ type: 'mouseup', button: pressedButtonRef.current! });
      }

      isLongPressRef.current = false;
      hasMoveRef.current = false;
      isDraggingRef.current = false;
      pressedButtonRef.current = null;
      emitMobileCursorHide(0);
    }

    // get mouse coordinate
    function getCoordinate(event: any) {
      const { x, y } = getCorrectedCoords(event.clientX, event.clientY);

      const finalX = Math.max(0, Math.min(1, x));
      const finalY = Math.max(0, Math.min(1, y));

      const hexX = Math.round(HID_ABSOLUTE_MAX * finalX);
      const hexY = Math.round(HID_ABSOLUTE_MAX * finalY);

      return { x: hexX, y: hexY, xRatio: finalX, yRatio: finalY };
    }

    function getRelativeTouchCoordinate(touch: Touch) {
      const rect = screenElement.getBoundingClientRect();
      const clientDeltaX = touch.clientX - lastTouchPosRef.current.x;
      const clientDeltaY = touch.clientY - lastTouchPosRef.current.y;
      const deltaX = rect.width > 0 ? clientDeltaX / rect.width : 0;
      const deltaY = rect.height > 0 ? clientDeltaY / rect.height : 0;
      lastTouchPosRef.current = { x: touch.clientX, y: touch.clientY };

      const currentX = lastPosRef.current.x / HID_ABSOLUTE_MAX;
      const currentY = lastPosRef.current.y / HID_ABSOLUTE_MAX;
      followCursor(clientDeltaX, clientDeltaY, rect, currentX, currentY);
      const finalX = Math.max(0, Math.min(1, currentX + deltaX));
      const finalY = Math.max(0, Math.min(1, currentY + deltaY));

      return ratioToCoordinate(finalX, finalY);
    }

    function emitCurrentMobileCursorPosition() {
      emitMobileCursorAbsolute(
        lastPosRef.current.x / HID_ABSOLUTE_MAX,
        lastPosRef.current.y / HID_ABSOLUTE_MAX
      );
    }

    function ratioToCoordinate(xRatio: number, yRatio: number) {
      const finalX = Math.max(0, Math.min(1, xRatio));
      const finalY = Math.max(0, Math.min(1, yRatio));
      const x = Math.round(HID_ABSOLUTE_MAX * finalX);
      const y = Math.round(HID_ABSOLUTE_MAX * finalY);

      return { x, y, xRatio: finalX, yRatio: finalY };
    }

    // Follow the cursor like the RustAdmin/RustDesk mouse mode: the cursor
    // moves on its own until it passes the centre of the visible area in the
    // direction of travel, then the image scrolls under it; once the image
    // edge meets the view edge the scroll is clamped and the cursor moves on
    // towards the edge.
    function followCursor(
      deltaX: number,
      deltaY: number,
      screenRect: DOMRect,
      xRatio: number,
      yRatio: number
    ) {
      if (!scrollContainer) return;
      if (deltaX === 0 && deltaY === 0) return;

      const view = scrollContainer.getBoundingClientRect();
      const cursorX = screenRect.left + screenRect.width * xRatio;
      const cursorY = screenRect.top + screenRect.height * yRatio;
      const centerX = view.left + view.width / 2;
      const centerY = view.top + view.height / 2;
      const maxLeft = Math.max(0, scrollContainer.scrollWidth - scrollContainer.clientWidth);
      const maxTop = Math.max(0, scrollContainer.scrollHeight - scrollContainer.clientHeight);

      if (
        (deltaX > 0 && cursorX + deltaX > centerX) ||
        (deltaX < 0 && cursorX + deltaX < centerX)
      ) {
        scrollContainer.scrollLeft = clamp(scrollContainer.scrollLeft + deltaX, 0, maxLeft);
      }
      if (
        (deltaY > 0 && cursorY + deltaY > centerY) ||
        (deltaY < 0 && cursorY + deltaY < centerY)
      ) {
        scrollContainer.scrollTop = clamp(scrollContainer.scrollTop + deltaY, 0, maxTop);
      }
    }

    function startPinchZoom(touches: TouchList) {
      clearLongPress();
      emitMobileCursorHide(0);

      const metrics = getPinchMetrics(touches);
      if (!metrics) return;

      pinchDistanceRef.current = metrics.distance;
      pinchScaleRef.current = currentVideoScaleRef.current;
    }

    function updatePinchZoom(touches: TouchList) {
      const metrics = getPinchMetrics(touches);
      if (!metrics) return;

      if (pinchDistanceRef.current === null || pinchDistanceRef.current <= 0) {
        startPinchZoom(touches);
        return;
      }

      const nextScale = clamp(
        pinchScaleRef.current * (metrics.distance / pinchDistanceRef.current),
        MIN_TOUCH_ZOOM,
        MAX_TOUCH_ZOOM
      );

      if (Math.abs(nextScale - currentVideoScaleRef.current) < 0.01) {
        return;
      }

      setZoomAtPoint(nextScale, metrics.centerX, metrics.centerY);
    }

    function clearPinchZoom() {
      pinchDistanceRef.current = null;
      pinchScaleRef.current = currentVideoScaleRef.current;
    }

    function clearLongPress() {
      if (longPressTimerRef.current) {
        clearTimeout(longPressTimerRef.current);
        longPressTimerRef.current = null;
      }
    }

    function setZoomAtPoint(nextScale: number, clientX: number, clientY: number) {
      const previousScale = currentVideoScaleRef.current || 1;
      const roundedScale = Math.round(nextScale * 100) / 100;

      let nextScrollLeft: number | null = null;
      let nextScrollTop: number | null = null;

      if (scrollContainer && previousScale > 0) {
        const rect = scrollContainer.getBoundingClientRect();
        const localX = clientX - rect.left;
        const localY = clientY - rect.top;
        const ratio = roundedScale / previousScale;

        nextScrollLeft = (scrollContainer.scrollLeft + localX) * ratio - localX;
        nextScrollTop = (scrollContainer.scrollTop + localY) * ratio - localY;
      }

      currentVideoScaleRef.current = roundedScale;
      setVideoScale(roundedScale);
      storage.setVideoScale(roundedScale);

      if (scrollContainer && nextScrollLeft !== null && nextScrollTop !== null) {
        window.requestAnimationFrame(() => {
          window.requestAnimationFrame(() => {
            scrollContainer.scrollLeft = clamp(
              nextScrollLeft,
              0,
              Math.max(0, scrollContainer.scrollWidth - scrollContainer.clientWidth)
            );
            scrollContainer.scrollTop = clamp(
              nextScrollTop,
              0,
              Math.max(0, scrollContainer.scrollHeight - scrollContainer.clientHeight)
            );
          });
        });
      }
    }

    function getCorrectedCoords(clientX: number, clientY: number) {
      return pointInPicture(
        clientX,
        clientY,
        mouseTarget.getBoundingClientRect(),
        pictureSize(mouseTarget)
      );
    }

    return () => {
      mouseTarget.removeEventListener('mousemove', handleMouseMove);
      mouseTarget.removeEventListener('mousedown', handleMouseDown);
      mouseTarget.removeEventListener('mouseup', handleMouseUp);
      mouseTarget.removeEventListener('wheel', handleWheel);
      mouseTarget.removeEventListener('click', disableEvent);
      mouseTarget.removeEventListener('contextmenu', disableEvent);
      touchTarget.removeEventListener('touchstart', handleTouchStart);
      touchTarget.removeEventListener('touchmove', handleTouchMove);
      touchTarget.removeEventListener('touchend', handleTouchEnd);
      touchTarget.removeEventListener('touchcancel', handleTouchCancel);
      viewObserver?.disconnect();
      for (const style of previousGestureStyles) {
        style.element.style.touchAction = style.touchAction;
        style.element.style.overscrollBehavior = style.overscrollBehavior;
        style.element.style.userSelect = style.userSelect;
        style.element.style.webkitUserSelect = style.webkitUserSelect;
      }

      if (longPressTimerRef.current) {
        clearTimeout(longPressTimerRef.current);
      }
    };
  }, [isTouchLayout, resolution, scrollDirection, scrollInterval, setVideoScale]);

  // Mouse event handler
  function handleMouseEvent(event: MouseAbsoluteEvent) {
    let report: Uint8Array;
    const mouse = mouseRef.current;

    switch (event.type) {
      case 'mousedown':
        mouse.buttonDown(event.button);
        report = mouse.buildButtonReport(lastPosRef.current.x, lastPosRef.current.y);
        break;
      case 'mouseup':
        mouse.buttonUp(event.button);
        report = mouse.buildButtonReport(lastPosRef.current.x, lastPosRef.current.y);
        break;
      case 'wheel':
        report = mouse.buildReport(lastPosRef.current.x, lastPosRef.current.y, event.deltaY);
        break;
      case 'move':
        report = mouse.buildReport(event.x, event.y);
        lastPosRef.current = { x: event.x, y: event.y };
        break;
      default:
        report = mouse.buildReport(lastPosRef.current.x, lastPosRef.current.y);
        break;
    }

    sendReport(report);
  }

  function sendReport(report: Uint8Array) {
    if (restricted) {
      reportHid({ mode: 'absolute_mouse', data: Array.from(report) });
      return;
    }
    const data = new Uint8Array([MessageEvent.Mouse, ...report]);
    client.send(data);
  }

  // disable default events
  function disableEvent(event: any) {
    event.preventDefault();
    event.stopPropagation();
  }

  return <></>;
};

function uniqueElements(elements: Array<HTMLElement | null>): HTMLElement[] {
  return Array.from(
    new Set(elements.filter((element): element is HTMLElement => Boolean(element)))
  );
}

function getPinchMetrics(touches: TouchList): {
  distance: number;
  centerX: number;
  centerY: number;
} | null {
  if (touches.length < 2) return null;

  const first = touches[0];
  const second = touches[1];
  const deltaX = second.clientX - first.clientX;
  const deltaY = second.clientY - first.clientY;

  return {
    distance: Math.hypot(deltaX, deltaY),
    centerX: (first.clientX + second.clientX) / 2,
    centerY: (first.clientY + second.clientY) / 2
  };
}

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value));
}
