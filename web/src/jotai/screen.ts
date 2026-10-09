import { atom } from 'jotai';

import { Resolution } from '@/types';
import { DEFAULT_UPSCALE, readUpscale, type UpscaleSetting } from '@/lib/upscale/settings.ts';
import type { UpscaleStop } from '@/lib/upscale/watchdog.ts';

export const isHdmiEnabledAtom = atom(true);

// video mode
// direct: stream H.264 over HTTP
// h264: stream H.264 over WebRTC
// mjpeg: stream JPEG over HTTP
export const videoModeAtom = atom('');

export const videoScaleAtom = atom<number>(1.0);

// browser screen resolution
export const resolutionAtom = atom<Resolution | null>(null);

// Bumped after an applied capture change (fps, quality, resolution). The owner
// restarts the streamer for it, which ends the open MJPEG response without an
// error event, so the player must request the stream again.
export const videoStreamGenerationAtom = atom(0);

function storedUpscale(): UpscaleSetting {
  try {
    return readUpscale(window.localStorage);
  } catch {
    return DEFAULT_UPSCALE;
  }
}

// Upscaling of the picture in this browser (docs/design/screen-upscaling-v1.md).
export const upscaleAtom = atom<UpscaleSetting>(storedUpscale());

// Why upscaling stopped for this page load, null while it may run: no
// WebGL2 where the view draws, its context lost, or frames lost to it.
export const upscaleStoppedAtom = atom<UpscaleStop | null>(null);
