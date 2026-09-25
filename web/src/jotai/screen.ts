import { atom } from 'jotai';

import { Resolution } from '@/types';

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
