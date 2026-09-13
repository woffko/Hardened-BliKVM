import type { CapabilityManifest, VideoTransport } from '@/types/capabilities.ts';

export type VideoUiMode = 'mjpeg' | 'direct' | 'h264';

/**
 * Selects only a transport the manifest actually advertises. The list starts
 * with loopback MJPEG; WebRTC is never chosen until it is advertised with
 * stable parity evidence.
 */
export function selectVideoTransport(
  manifest: CapabilityManifest | null,
  uiMode: VideoUiMode
): VideoTransport | null {
  if (!manifest) {
    return null;
  }
  const transports = manifest.video.transports;
  const candidates: VideoTransport[] =
    uiMode === 'mjpeg' ? ['mjpeg'] : uiMode === 'h264' ? ['h264_direct', 'mjpeg'] : ['mjpeg'];
  for (const candidate of candidates) {
    if (transports.includes(candidate)) {
      return candidate;
    }
  }
  const preferred = manifest.video.preferredTransport;
  if (preferred && transports.includes(preferred) && preferred === 'mjpeg') {
    return preferred;
  }
  return null;
}

export function isWebRtcAdvertised(manifest: CapabilityManifest | null) {
  return Boolean(
    manifest?.video.transports.some(
      (transport) => transport === 'web_rtc_janus' || transport === 'web_rtc_native'
    )
  );
}
