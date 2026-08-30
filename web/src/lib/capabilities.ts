import type { CapabilityManifest, FeatureStatus, VideoTransport } from '@/types/capabilities.ts';

export type UiVideoMode = 'mjpeg' | 'direct' | 'h264';
export type CapabilityMenuItem =
  | 'image'
  | 'download'
  | 'terminal'
  | 'script'
  | 'wol'
  | 'picoclaw'
  | 'power'
  | 'fullscreen'
  | 'collapse';
export type CapabilitySettingsTab =
  | 'about'
  | 'appearance'
  | 'device'
  | 'system'
  | 'tailscale'
  | 'update'
  | 'account';

export function isAvailable(status: FeatureStatus) {
  return status.availability !== 'unavailable';
}

export function isPhysicallyVerified(status: FeatureStatus) {
  return status.evidence === 'physical_verified';
}

function boardFeatureAvailable(manifest: CapabilityManifest | null, name: string) {
  const feature = manifest?.board.features[name];
  return feature ? isAvailable(feature) : false;
}

export function supportsMenuItem(
  manifest: CapabilityManifest | null,
  item: CapabilityMenuItem
): boolean {
  switch (item) {
    case 'image':
      return manifest ? isAvailable(manifest.virtualMedia.status) : false;
    case 'download':
      return manifest ? isAvailable(manifest.virtualMedia.remoteDownload) : false;
    case 'power':
      return Boolean(
        manifest && isAvailable(manifest.power.status) && manifest.power.actions.length > 0
      );
    case 'terminal':
      return boardFeatureAvailable(manifest, 'terminal');
    case 'script':
      return boardFeatureAvailable(manifest, 'scripts');
    case 'wol':
      return boardFeatureAvailable(manifest, 'wake_on_lan');
    case 'picoclaw':
      return boardFeatureAvailable(manifest, 'picoclaw');
    case 'fullscreen':
    case 'collapse':
      return true;
  }
}

export function supportsSettingsTab(
  manifest: CapabilityManifest | null,
  tab: CapabilitySettingsTab
): boolean {
  switch (tab) {
    case 'about':
    case 'appearance':
    case 'account':
      return true;
    case 'device':
      return boardFeatureAvailable(manifest, 'device_settings');
    case 'system':
      return boardFeatureAvailable(manifest, 'system_settings');
    case 'tailscale':
      return boardFeatureAvailable(manifest, 'tailscale');
    case 'update':
      return Boolean(
        manifest &&
        (isAvailable(manifest.system.signedApplicationUpdates) ||
          isAvailable(manifest.system.signedSystemUpdates))
      );
  }
}

export function supportsVideoTransport(
  manifest: CapabilityManifest | null,
  transport: VideoTransport
) {
  return Boolean(
    manifest && isAvailable(manifest.video.status) && manifest.video.transports.includes(transport)
  );
}

export function supportedUiVideoModes(manifest: CapabilityManifest | null): UiVideoMode[] {
  const modes: UiVideoMode[] = [];
  if (supportsVideoTransport(manifest, 'mjpeg')) modes.push('mjpeg');
  if (supportsVideoTransport(manifest, 'h264_direct')) modes.push('direct');
  if (supportsVideoTransport(manifest, 'web_rtc_native')) modes.push('h264');
  return modes.length ? modes : ['mjpeg'];
}

export function normalizeVideoMode(
  manifest: CapabilityManifest | null,
  requested: string | null | undefined
): UiVideoMode {
  const supported = supportedUiVideoModes(manifest);
  return supported.includes(requested as UiVideoMode) ? (requested as UiVideoMode) : supported[0];
}
