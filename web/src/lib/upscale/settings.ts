/**
 * The upscaling setting of this browser and the rules it is applied by
 * (docs/design/screen-upscaling-v1.md): Off or Auto, and a sharpness. Auto
 * enlarges the picture sharply only where the browser draws with a GPU and
 * the picture is shown more than 1.1 times its size; otherwise the view
 * draws as it does without upscaling.
 */
import type { UpscaleMode } from './upscaler.ts';

export type Sharpness = Exclude<UpscaleMode, 'plain'>;
export type Size = { width: number; height: number };
/**
 * How large the picture is shown while upscaling: as large as the window
 * lets it (beyond its own size), or at the target's own resolution, which
 * the operator gives (2560x1440 shows a 1920x1080 capture at 133 %).
 */
export type PictureSize = 'fill' | 'target';
export type UpscaleSetting = {
  mode: 'off' | 'auto';
  sharpness: Sharpness;
  size: PictureSize;
  target: Size | null;
};

export const DEFAULT_UPSCALE: UpscaleSetting = {
  mode: 'off',
  sharpness: 'medium',
  size: 'fill',
  target: null
};
const STORAGE_KEY = 'hardened-blikvm-upscale';
const SHARPNESSES: Sharpness[] = ['low', 'medium', 'high'];

/** Auto upscales a picture shown at more than this many times its size. */
export const AUTO_ENLARGEMENT = 1.1;
/** The largest canvas buffer; a picture drawn larger is stretched by the browser. */
export const MAX_BUFFER = { width: 3840, height: 2160 };

type Storage = { getItem(key: string): string | null; setItem(key: string, value: string): void };

/** A target resolution that makes sense, or null. */
export function targetSize(value: unknown): Size | null {
  const size = value as Partial<Size> | null;
  const fine = (n: unknown) =>
    typeof n === 'number' && Number.isInteger(n) && n >= 320 && n <= 16384;
  return size && fine(size.width) && fine(size.height)
    ? { width: size.width as number, height: size.height as number }
    : null;
}

/** The setting this browser keeps, or the default where it keeps none that makes sense. */
export function readUpscale(storage: Storage): UpscaleSetting {
  try {
    const value = JSON.parse(storage.getItem(STORAGE_KEY) ?? 'null') as Partial<UpscaleSetting>;
    if (value && (value.mode === 'off' || value.mode === 'auto')) {
      const sharpness = SHARPNESSES.includes(value.sharpness as Sharpness)
        ? (value.sharpness as Sharpness)
        : DEFAULT_UPSCALE.sharpness;
      const target = targetSize(value.target);
      const size = value.size === 'target' && target ? 'target' : 'fill';
      return { mode: value.mode, sharpness, size, target };
    }
  } catch {
    // Not ours, or damaged: the default.
  }
  return DEFAULT_UPSCALE;
}

export function writeUpscale(storage: Storage, setting: UpscaleSetting): void {
  try {
    storage.setItem(STORAGE_KEY, JSON.stringify(setting));
  } catch {
    // A browser that keeps nothing keeps the setting for this page only.
  }
}

/**
 * The canvas buffer for a picture drawn `width` x `height` CSS pixels on a
 * display of `ratio`: its device pixels, at most MAX_BUFFER, of the same
 * shape.
 */
export function bufferSize(
  width: number,
  height: number,
  ratio: number
): { width: number; height: number } {
  const scale = Math.min(
    1,
    MAX_BUFFER.width / (width * ratio),
    MAX_BUFFER.height / (height * ratio)
  );
  return {
    width: Math.max(1, Math.round(width * ratio * scale)),
    height: Math.max(1, Math.round(height * ratio * scale))
  };
}

/**
 * How a frame of `frameWidth` pixels is drawn in a picture shown
 * `shownWidth` x `shownHeight` CSS pixels on a display of `ratio`: enlarged
 * with `sharpness` into a buffer of the shown device pixels when it is
 * enlarged more than AUTO_ENLARGEMENT times, else plainly at its own size,
 * the browser scaling the canvas as it does any picture.
 */
export function drawing(
  frameWidth: number,
  frameHeight: number,
  shownWidth: number,
  shownHeight: number,
  ratio: number,
  sharpness: Sharpness
): { mode: UpscaleMode; width: number; height: number } {
  const enlargement = (shownWidth * ratio) / frameWidth;
  if (!(enlargement > AUTO_ENLARGEMENT)) {
    return { mode: 'plain', width: frameWidth, height: frameHeight };
  }
  return { mode: sharpness, ...bufferSize(shownWidth, shownHeight, ratio) };
}

/**
 * How the screen fits the picture in its view: `fit` at most its own size
 * (the plain picture, as always), `fill` as large as the view lets it, or
 * at a target's resolution.
 */
export type Fit = 'fit' | 'fill' | { target: Size };

/** The scale the picture is laid out at in a view of `view` CSS pixels, before the zoom. */
export function fitScale(view: Size, picture: Size, fit: Fit): number {
  const whole = Math.min(view.width / picture.width, view.height / picture.height);
  const scale =
    fit === 'fit' ? Math.min(whole, 1) : fit === 'fill' ? whole : fit.target.width / picture.width;
  return Math.max(0.05, scale);
}

/** The fit of the picture for a setting: the plain picture's unless it is upscaled. */
export function fitFor(setting: UpscaleSetting, upscaling: boolean): Fit {
  if (!upscaling) return 'fit';
  return setting.size === 'target' && setting.target ? { target: setting.target } : 'fill';
}
