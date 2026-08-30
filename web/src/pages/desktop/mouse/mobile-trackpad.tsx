import { useEffect, useRef } from 'react';
import { useAtomValue } from 'jotai';

import { MouseReportRelative } from '@/lib/mouse.ts';
import { client, MessageEvent } from '@/lib/websocket.ts';
import { pointerSensitivityAtom, scrollDirectionAtom, scrollIntervalAtom } from '@/jotai/mouse.ts';
import { resolutionAtom } from '@/jotai/screen.ts';

import {
  emitMobileCursorHide,
  emitMobileCursorRelative,
  emitMobileCursorShow
} from './mobile-cursor-events.ts';
import {
  getMobileTrackpadMovementScale,
  hasAccumulatedMovement,
  RELATIVE_DRAIN_INTERVAL_MS,
  resetAccumulatedMovement,
  takeAccumulatedMovement
} from './movement-scale.ts';

enum MouseButton {
  Left = 0,
  Middle = 1,
  Right = 2
}

type Point = {
  x: number;
  y: number;
};

const TAP_THRESHOLD = 10;
const TAP_MAX_MS = 300;
const LONG_PRESS_MS = 650;

export const MobileTrackpad = () => {
  const scrollDirection = useAtomValue(scrollDirectionAtom);
  const scrollInterval = useAtomValue(scrollIntervalAtom);
  const pointerSensitivity = useAtomValue(pointerSensitivityAtom);
  const resolution = useAtomValue(resolutionAtom);

  const mouseRef = useRef(new MouseReportRelative());
  const activePointersRef = useRef(new Map<number, Point>());
  const startRef = useRef<Point>({ x: 0, y: 0 });
  const lastRef = useRef<Point>({ x: 0, y: 0 });
  const startTimeRef = useRef(0);
  const movedRef = useRef(false);
  const longPressTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const longPressTriggeredRef = useRef(false);
  const twoFingerRef = useRef(false);
  const twoFingerMovedRef = useRef(false);
  const lastScrollTimeRef = useRef(0);
  const moveAccumulatorRef = useRef({ x: 0, y: 0 });
  const drainTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const screen = document.getElementById('screen') as HTMLElement | null;
    if (!screen) return;

    const target = (document.getElementById('kvm-pointer-surface') ?? screen) as HTMLElement;
    const scrollContainer = screen.closest('[data-kvm-screen-scroll]') as HTMLElement | null;
    const gestureElements = uniqueElements([target, screen, scrollContainer]);
    const previousGestureStyles = gestureElements.map((element) => ({
      element,
      touchAction: element.style.touchAction,
      overscrollBehavior: element.style.overscrollBehavior,
      userSelect: element.style.userSelect,
      webkitUserSelect: element.style.webkitUserSelect
    }));
    const pointerOptions: AddEventListenerOptions = { capture: true };
    const activePointers = activePointersRef.current;
    const mouse = mouseRef.current;

    for (const element of gestureElements) {
      element.style.touchAction = 'none';
      element.style.overscrollBehavior = 'contain';
      element.style.userSelect = 'none';
      element.style.webkitUserSelect = 'none';
    }

    target.addEventListener('pointerdown', handlePointerDown, pointerOptions);
    target.addEventListener('contextmenu', disableEvent, true);
    window.addEventListener('pointermove', handlePointerMove, pointerOptions);
    window.addEventListener('pointerup', handlePointerUp, pointerOptions);
    window.addEventListener('pointercancel', handlePointerCancel, pointerOptions);

    function handlePointerDown(event: PointerEvent) {
      disableEvent(event);
      try {
        target.setPointerCapture?.(event.pointerId);
      } catch {
        // Window-level listeners keep the gesture alive if capture is unavailable.
      }

      activePointersRef.current.set(event.pointerId, getPoint(event));
      clearLongPress();
      startTimeRef.current = Date.now();
      longPressTriggeredRef.current = false;
      movedRef.current = false;
      twoFingerMovedRef.current = false;

      const activePointers = getActivePointers();
      if (activePointers.length >= 2) {
        twoFingerRef.current = true;
        emitMobileCursorHide(0);
        const center = getCenter(activePointers[0], activePointers[1]);
        startRef.current = center;
        lastRef.current = center;
        return;
      }

      twoFingerRef.current = false;
      emitMobileCursorShow();
      const point = getPoint(event);
      startRef.current = point;
      lastRef.current = point;

      longPressTimerRef.current = setTimeout(() => {
        if (movedRef.current || twoFingerRef.current) return;

        longPressTriggeredRef.current = true;
        navigator.vibrate?.(40);
        click(MouseButton.Right);
      }, LONG_PRESS_MS);
    }

    function handlePointerMove(event: PointerEvent) {
      if (!activePointersRef.current.has(event.pointerId)) return;
      disableEvent(event);

      if (activePointersRef.current.size <= 1 && !twoFingerRef.current) {
        const events = getCoalescedPointerEvents(event);
        for (const pointerEvent of events) {
          if (!activePointersRef.current.has(event.pointerId)) return;
          const point = getPoint(pointerEvent);
          activePointersRef.current.set(event.pointerId, point);
          processSinglePointerMove(point);
        }
        return;
      }

      activePointersRef.current.set(event.pointerId, getPoint(event));
      const activePointers = getActivePointers();

      if (activePointers.length >= 2) {
        clearLongPress();
        twoFingerRef.current = true;
        emitMobileCursorHide(0);
        const center = getCenter(activePointers[0], activePointers[1]);
        const deltaY = center.y - lastRef.current.y;

        if (Math.abs(deltaY) > 6) {
          sendWheel(deltaY > 0 ? 1 : -1);
          twoFingerMovedRef.current = true;
          lastRef.current = center;
        }
        return;
      }
    }

    function processSinglePointerMove(point: Point) {
      const activePointers = getActivePointers();
      if (activePointers.length !== 1 || twoFingerRef.current) return;

      const totalX = point.x - startRef.current.x;
      const totalY = point.y - startRef.current.y;
      const distance = Math.hypot(totalX, totalY);
      const deltaX = point.x - lastRef.current.x;
      const deltaY = point.y - lastRef.current.y;

      if (distance > TAP_THRESHOLD) {
        movedRef.current = true;
        clearLongPress();
      }

      if (deltaX !== 0 || deltaY !== 0) {
        const movementScale = getMobileTrackpadMovementScale(resolution);
        const remoteDeltaX = deltaX * pointerSensitivity * movementScale;
        const remoteDeltaY = deltaY * pointerSensitivity * movementScale;

        emitMobileCursorRelative(remoteDeltaX, remoteDeltaY, resolution);
        queueRelativeMove(remoteDeltaX, remoteDeltaY);
        lastRef.current = point;
      }
    }

    function handlePointerUp(event: PointerEvent) {
      if (!activePointersRef.current.has(event.pointerId)) return;
      disableEvent(event);
      try {
        target.releasePointerCapture?.(event.pointerId);
      } catch {
        // Capture may already be gone on Android Chrome; the gesture still ends here.
      }
      activePointersRef.current.delete(event.pointerId);

      if (activePointersRef.current.size > 0) {
        return;
      }

      finishGesture();
      emitMobileCursorHide();
    }

    function handlePointerCancel(event: PointerEvent) {
      if (!activePointersRef.current.has(event.pointerId)) return;
      disableEvent(event);
      try {
        target.releasePointerCapture?.(event.pointerId);
      } catch {
        // Capture may already be gone on Android Chrome.
      }
      activePointersRef.current.delete(event.pointerId);

      if (activePointersRef.current.size > 0) {
        return;
      }

      clearLongPress();
      clearMovementDrain();
      resetAccumulatedMovement(moveAccumulatorRef.current);
      sendReport(mouseRef.current.reset());
      resetGesture();
      emitMobileCursorHide(0);
    }

    function finishGesture() {
      clearLongPress();
      const elapsed = Date.now() - startTimeRef.current;

      if (twoFingerRef.current) {
        if (!twoFingerMovedRef.current && elapsed < TAP_MAX_MS) {
          click(MouseButton.Right);
        }
        resetGesture();
        return;
      }

      if (!movedRef.current && !longPressTriggeredRef.current && elapsed < TAP_MAX_MS) {
        click(MouseButton.Left);
      }

      resetGesture();
    }

    function resetGesture() {
      movedRef.current = false;
      twoFingerRef.current = false;
      twoFingerMovedRef.current = false;
      longPressTriggeredRef.current = false;
      clearMovementDrain();
      resetAccumulatedMovement(moveAccumulatorRef.current);
    }

    function clearLongPress() {
      if (!longPressTimerRef.current) return;
      clearTimeout(longPressTimerRef.current);
      longPressTimerRef.current = null;
    }

    function click(button: MouseButton) {
      const mouse = mouseRef.current;
      mouse.buttonDown(button);
      sendReport(mouse.buildButtonReport());
      setTimeout(() => {
        mouse.buttonUp(button);
        sendReport(mouse.buildButtonReport());
      }, 45);
    }

    function sendMove(deltaX: number, deltaY: number) {
      sendReport(mouseRef.current.buildReport(deltaX, deltaY));
    }

    function queueRelativeMove(deltaX: number, deltaY: number) {
      const move = takeAccumulatedMovement(moveAccumulatorRef.current, deltaX, deltaY);
      if (move) {
        sendMove(move.x, move.y);
      }
      scheduleMovementDrain();
    }

    function scheduleMovementDrain() {
      if (drainTimerRef.current !== null || !hasAccumulatedMovement(moveAccumulatorRef.current)) {
        return;
      }

      drainTimerRef.current = setTimeout(() => {
        drainTimerRef.current = null;
        const move = takeAccumulatedMovement(moveAccumulatorRef.current, 0, 0);
        if (move) {
          sendMove(move.x, move.y);
        }
        scheduleMovementDrain();
      }, RELATIVE_DRAIN_INTERVAL_MS);
    }

    function clearMovementDrain() {
      if (drainTimerRef.current === null) return;
      clearTimeout(drainTimerRef.current);
      drainTimerRef.current = null;
    }

    function sendWheel(direction: number) {
      const currentTime = Date.now();
      if (currentTime - lastScrollTimeRef.current < scrollInterval) {
        return;
      }

      sendReport(mouseRef.current.buildReport(0, 0, direction * scrollDirection));
      lastScrollTimeRef.current = currentTime;
    }

    function sendReport(report: Uint8Array) {
      const data = new Uint8Array([MessageEvent.Mouse, ...report]);
      client.send(data);
    }

    function disableEvent(event: Event) {
      event.preventDefault();
      event.stopPropagation();
    }

    function getPoint(event: PointerEvent): Point {
      return { x: event.clientX, y: event.clientY };
    }

    function getActivePointers(): Point[] {
      return Array.from(activePointersRef.current.values());
    }

    function getCenter(first: Point, second: Point): Point {
      return {
        x: (first.x + second.x) / 2,
        y: (first.y + second.y) / 2
      };
    }

    function getCoalescedPointerEvents(event: PointerEvent): PointerEvent[] {
      const events = event.getCoalescedEvents?.();
      return events && events.length > 0 ? events : [event];
    }

    return () => {
      for (const style of previousGestureStyles) {
        style.element.style.touchAction = style.touchAction;
        style.element.style.overscrollBehavior = style.overscrollBehavior;
        style.element.style.userSelect = style.userSelect;
        style.element.style.webkitUserSelect = style.webkitUserSelect;
      }
      target.removeEventListener('pointerdown', handlePointerDown, pointerOptions);
      target.removeEventListener('contextmenu', disableEvent, true);
      window.removeEventListener('pointermove', handlePointerMove, pointerOptions);
      window.removeEventListener('pointerup', handlePointerUp, pointerOptions);
      window.removeEventListener('pointercancel', handlePointerCancel, pointerOptions);
      clearLongPress();
      clearMovementDrain();
      activePointers.clear();
      sendReport(mouse.reset());
      emitMobileCursorHide(0);
    };
  }, [scrollDirection, scrollInterval, pointerSensitivity, resolution]);

  return null;
};

function uniqueElements(elements: Array<HTMLElement | null>): HTMLElement[] {
  return Array.from(
    new Set(elements.filter((element): element is HTMLElement => Boolean(element)))
  );
}
