export type Availability = 'unavailable' | 'read_only' | 'read_write';

export type EvidenceLevel =
  | 'none'
  | 'user_reported'
  | 'source_verified'
  | 'runtime_verified'
  | 'physical_verified';

export type FeatureStatus = {
  availability: Availability;
  evidence: EvidenceLevel;
};

export type VideoTransport =
  | 'mjpeg'
  | 'h264_direct'
  | 'web_rtc_janus'
  | 'web_rtc_native'
  | 'web_rtc_janus_h264';
export type HidMode = 'keyboard' | 'absolute_mouse' | 'relative_mouse';
export type PowerAction = 'power_short' | 'power_long' | 'reset';
export type VirtualMediaMode = 'cd_rom' | 'removable_disk';

export type CapabilityManifest = {
  schemaVersion: 1;
  product: {
    name: string;
    shortName: string;
    tagline: string;
  };
  platform: {
    id: string;
    board: string;
    architecture: string;
    operatingSystem: string;
  };
  video: {
    status: FeatureStatus;
    transports: VideoTransport[];
    preferredTransport: VideoTransport | null;
    maxWidth: number | null;
    maxHeight: number | null;
    audio: FeatureStatus;
    settings: ('fps' | 'quality' | 'resolution' | 'frame_detect')[];
  };
  hid: {
    status: FeatureStatus;
    modes: HidMode[];
  };
  power: {
    status: FeatureStatus;
    actions: PowerAction[];
    ledState: FeatureStatus;
  };
  virtualMedia: {
    status: FeatureStatus;
    modes: VirtualMediaMode[];
    resumableUpload: FeatureStatus;
    remoteDownload: FeatureStatus;
  };
  board: {
    features: Record<string, FeatureStatus>;
  };
  system: {
    status: FeatureStatus;
    readOnlyRootfs: boolean;
    maintenanceWriteMode: boolean;
    actions: FeatureStatus;
    signedApplicationUpdates: FeatureStatus;
    signedSystemUpdates: FeatureStatus;
    automaticRollback: FeatureStatus;
  };
  network: {
    status: FeatureStatus;
    dns: FeatureStatus;
    ipv6: FeatureStatus;
    wol: FeatureStatus;
  };
};
