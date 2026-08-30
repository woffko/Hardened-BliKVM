export type InventoryBackend = 'kvmd_transition' | 'native';

export type ComponentObservation = {
  enabled?: boolean | null;
  online?: boolean | null;
  busy?: boolean | null;
  connected?: boolean | null;
};

export type RuntimeInventory = {
  schemaVersion: 1;
  platformId: string;
  backend: InventoryBackend;
  versions: {
    kvmd?: string | null;
    streamer?: string | null;
  };
  video: ComponentObservation;
  hid: ComponentObservation;
  power: ComponentObservation;
  virtualMedia: ComponentObservation;
  powerLed?: boolean | null;
  janusEnabled?: boolean | null;
};
