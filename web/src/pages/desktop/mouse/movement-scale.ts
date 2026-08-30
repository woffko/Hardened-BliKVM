import type { Resolution } from '@/types';

const BASE_MOVEMENT_SCALE = 1;
const MAX_RELATIVE_PACKET_DELTA = 8;

export const RELATIVE_DRAIN_INTERVAL_MS = 4;

export function getResolutionMovementScale(resolution: Resolution | null): number {
  if (!resolution?.width || !resolution?.height) {
    return BASE_MOVEMENT_SCALE;
  }

  if (resolution.width <= 0 || resolution.height <= 0) {
    return BASE_MOVEMENT_SCALE;
  }

  return BASE_MOVEMENT_SCALE;
}

export function getMobileTrackpadMovementScale(resolution: Resolution | null): number {
  if (!resolution?.width || !resolution?.height) {
    return BASE_MOVEMENT_SCALE;
  }

  const screen = document.getElementById('screen');
  const rect = screen?.getBoundingClientRect();
  if (!rect || rect.width <= 0 || rect.height <= 0) {
    return BASE_MOVEMENT_SCALE;
  }

  const scaleX = resolution.width / rect.width;
  const scaleY = resolution.height / rect.height;
  return clamp(Math.max(scaleX, scaleY), BASE_MOVEMENT_SCALE, 8);
}

export function takeAccumulatedMovement(
  accumulator: { x: number; y: number },
  deltaX: number,
  deltaY: number,
  maxDelta: number = MAX_RELATIVE_PACKET_DELTA
): { x: number; y: number } | null {
  accumulator.x += deltaX;
  accumulator.y += deltaY;

  let x = accumulator.x < 0 ? Math.ceil(accumulator.x) : Math.floor(accumulator.x);
  let y = accumulator.y < 0 ? Math.ceil(accumulator.y) : Math.floor(accumulator.y);

  if (x === 0 && y === 0) {
    return null;
  }

  if (maxDelta > 0) {
    x = clamp(x, -maxDelta, maxDelta);
    y = clamp(y, -maxDelta, maxDelta);
  }

  accumulator.x -= x;
  accumulator.y -= y;

  return { x, y };
}

export function hasAccumulatedMovement(accumulator: { x: number; y: number }): boolean {
  return Math.abs(accumulator.x) >= 1 || Math.abs(accumulator.y) >= 1;
}

export function resetAccumulatedMovement(accumulator: { x: number; y: number }): void {
  accumulator.x = 0;
  accumulator.y = 0;
}

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value));
}

export function autoPanScreenToPointer(clientX: number, clientY: number): void {
  const container = document.querySelector<HTMLElement>('[data-kvm-screen-scroll]');
  if (!container) return;

  const rect = container.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return;
  if (
    container.scrollWidth <= container.clientWidth &&
    container.scrollHeight <= container.clientHeight
  ) {
    return;
  }

  const edge = Math.min(84, Math.max(42, Math.min(rect.width, rect.height) * 0.16));
  const maxStep = 36;
  let left = 0;
  let top = 0;

  if (clientX < rect.left + edge) {
    left = -Math.ceil(((rect.left + edge - clientX) / edge) * maxStep);
  } else if (clientX > rect.right - edge) {
    left = Math.ceil(((clientX - (rect.right - edge)) / edge) * maxStep);
  }

  if (clientY < rect.top + edge) {
    top = -Math.ceil(((rect.top + edge - clientY) / edge) * maxStep);
  } else if (clientY > rect.bottom - edge) {
    top = Math.ceil(((clientY - (rect.bottom - edge)) / edge) * maxStep);
  }

  if (left !== 0 || top !== 0) {
    container.scrollBy({ left, top, behavior: 'auto' });
  }
}
