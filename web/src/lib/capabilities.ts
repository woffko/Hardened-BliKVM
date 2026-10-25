import type { CapabilityManifest, FeatureStatus, VideoTransport } from '@/types/capabilities.ts';

export type UiVideoMode = 'mjpeg' | 'direct' | 'h264' | 'vp8';
export type CapabilityMenuItem =
  | 'image'
  | 'download'
  | 'terminal'
  | 'script'
  | 'wol'
  | 'picoclaw'
  | 'power'
  | 'qualityMonitor'
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
export type CapabilitySystemSection = 'network' | 'time' | 'firewall' | 'systemLog';

export function isAvailable(status: FeatureStatus) {
  return status.availability !== 'unavailable';
}

export function isWritable(status: FeatureStatus) {
  return status.availability === 'read_write';
}

export function supportsRestrictedControls(manifest: CapabilityManifest | null) {
  return Boolean(manifest && isWritable(manifest.hid.status) && isWritable(manifest.power.status));
}

export function supportsPowerControl(manifest: CapabilityManifest | null) {
  return Boolean(manifest && isWritable(manifest.power.status));
}

export function usesBliApplicationUpdates(manifest: CapabilityManifest | null) {
  return manifest?.platform.id === 'bli-h616-pikvm';
}

/**
 * True when the hardened server backs virtual media. The GUI must then use the
 * opaque-ID `/api/virtual-media` endpoints instead of the legacy storage API.
 */
export function supportsHardenedMedia(manifest: CapabilityManifest | null) {
  return Boolean(
    manifest &&
    manifest.platform.id === 'bli-h616-pikvm' &&
    isAvailable(manifest.virtualMedia.status)
  );
}

export function isPhysicallyVerified(status: FeatureStatus) {
  return status.evidence === 'physical_verified';
}

function boardFeatureWritable(manifest: CapabilityManifest | null, name: string) {
  const feature = manifest?.board.features[name];
  return feature ? isWritable(feature) : false;
}

function boardFeatureAvailable(manifest: CapabilityManifest | null, name: string) {
  const feature = manifest?.board.features[name];
  return feature ? isAvailable(feature) : false;
}

export function supportsBoardFeature(manifest: CapabilityManifest | null, name: string) {
  return boardFeatureAvailable(manifest, name);
}

export function boardFeatureIsWritable(manifest: CapabilityManifest | null, name: string) {
  return boardFeatureWritable(manifest, name);
}

const SYSTEM_SECTION_FEATURE: Record<CapabilitySystemSection, string> = {
  network: 'system_network',
  time: 'system_time',
  firewall: 'system_firewall',
  systemLog: 'system_log'
};

export function supportsSystemSection(
  manifest: CapabilityManifest | null,
  section: CapabilitySystemSection
): boolean {
  if (section === 'network') return Boolean(manifest && isAvailable(manifest.network.status));
  return boardFeatureAvailable(manifest, SYSTEM_SECTION_FEATURE[section]);
}

export function systemSectionWritable(
  manifest: CapabilityManifest | null,
  section: CapabilitySystemSection
): boolean {
  if (section === 'network') return Boolean(manifest && isWritable(manifest.network.status));
  return boardFeatureWritable(manifest, SYSTEM_SECTION_FEATURE[section]);
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
      // The server advertises Wake-on-LAN as a network write.
      return Boolean(
        manifest &&
        ((manifest.network?.wol !== undefined && isWritable(manifest.network.wol)) ||
          boardFeatureWritable(manifest, 'wake_on_lan'))
      );
    case 'picoclaw':
      return boardFeatureWritable(manifest, 'picoclaw');
    case 'qualityMonitor':
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
      return boardFeatureAvailable(manifest, 'device_settings');
    case 'system':
      return boardFeatureAvailable(manifest, 'system_settings');
    case 'tailscale':
      return boardFeatureAvailable(manifest, 'tailscale');
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
  // H.264 over WebRTC: the reference's native path, or x264 through the
  // board's Janus gateway.
  if (
    supportsVideoTransport(manifest, 'web_rtc_native') ||
    supportsVideoTransport(manifest, 'web_rtc_janus_h264')
  )
    modes.push('h264');
  if (supportsVideoTransport(manifest, 'web_rtc_janus')) modes.push('vp8');
  return modes.length ? modes : ['mjpeg'];
}

export function normalizeVideoMode(
  manifest: CapabilityManifest | null,
  requested: string | null | undefined
): UiVideoMode {
  const supported = supportedUiVideoModes(manifest);
  return supported.includes(requested as UiVideoMode) ? (requested as UiVideoMode) : supported[0];
}
