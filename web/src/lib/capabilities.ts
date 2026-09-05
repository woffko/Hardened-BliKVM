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
  | 'diagnostics'
  | 'appearance'
  | 'device'
  | 'system'
  | 'tailscale'
  | 'update'
  | 'account';

export function isAvailable(status: FeatureStatus) {
  return status.availability !== 'unavailable';
}

export function isWritable(status: FeatureStatus) {
  return status.availability === 'read_write';
}

export function usesBliApplicationUpdates(manifest: CapabilityManifest | null) {
  return manifest?.platform.id === 'bli-h616-pikvm';
}

export function isPhysicallyVerified(status: FeatureStatus) {
  return status.evidence === 'physical_verified';
}

function boardFeatureWritable(manifest: CapabilityManifest | null, name: string) {
  const feature = manifest?.board.features[name];
  return feature ? isWritable(feature) : false;
}

export function supportsMenuItem(
  manifest: CapabilityManifest | null,
  item: CapabilityMenuItem
): boolean {
  switch (item) {
    case 'image':
      return manifest ? isWritable(manifest.virtualMedia.status) : false;
    case 'download':
      return manifest ? isWritable(manifest.virtualMedia.remoteDownload) : false;
    case 'power':
      return Boolean(
        manifest && isWritable(manifest.power.status) && manifest.power.actions.length > 0
      );
    case 'terminal':
      return boardFeatureWritable(manifest, 'terminal');
    case 'script':
      return boardFeatureWritable(manifest, 'scripts');
    case 'wol':
      return boardFeatureWritable(manifest, 'wake_on_lan');
    case 'picoclaw':
      return boardFeatureWritable(manifest, 'picoclaw');
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
    case 'diagnostics':
    case 'appearance':
    case 'account':
      return true;
    case 'device':
      return boardFeatureWritable(manifest, 'device_settings');
    case 'system':
      return boardFeatureWritable(manifest, 'system_settings');
    case 'tailscale':
      return boardFeatureWritable(manifest, 'tailscale');
    case 'update':
      return Boolean(
        manifest &&
        (usesBliApplicationUpdates(manifest)
          ? isAvailable(manifest.system.signedApplicationUpdates)
          : isWritable(manifest.system.signedApplicationUpdates) ||
            isWritable(manifest.system.signedSystemUpdates))
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
