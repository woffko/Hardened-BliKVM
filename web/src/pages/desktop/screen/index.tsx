import { useAtomValue } from 'jotai';

import { normalizeVideoMode, supportsVideoTransport } from '@/lib/capabilities.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { resolutionAtom, videoModeAtom } from '@/jotai/screen.ts';

import { H264Direct } from './h264-direct.tsx';
import { H264Webrtc } from './h264-webrtc.tsx';
import { Mjpeg } from './mjpeg.tsx';
import { Vp8Webrtc } from './vp8-webrtc.tsx';

export const Screen = () => {
  const requestedVideoMode = useAtomValue(videoModeAtom);
  const capabilities = useAtomValue(capabilitiesAtom);
  const videoMode = normalizeVideoMode(capabilities, requestedVideoMode);
  const resolution = useAtomValue(resolutionAtom);
  const streamKey = `${resolution?.width ?? 0}x${resolution?.height ?? 0}`;

  if (videoMode === 'mjpeg') {
    return <Mjpeg />;
  }

  if (videoMode === 'vp8') {
    return <Vp8Webrtc key={streamKey} codec="vp8" />;
  }

  // x264 through the board's Janus gateway, on the VP8 path's signaling.
  if (videoMode === 'h264' && supportsVideoTransport(capabilities, 'web_rtc_janus_h264')) {
    return <Vp8Webrtc key={`h264-${streamKey}`} codec="h264" />;
  }

  if (videoMode === 'direct') {
    return <H264Direct key={streamKey} />;
  }

  return <H264Webrtc key={streamKey} />;
};
