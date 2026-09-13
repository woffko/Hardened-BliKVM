import { http } from '@/lib/http.ts';

export type VirtualMediaMode = 'cd_rom' | 'removable_disk';

export type MediaImage = {
  imageId: number;
  imageName: string;
  size: number;
  cdromAllowed: boolean;
  removableAllowed: boolean;
  writableAllowed: boolean;
};

export type VirtualMediaCommand =
  | { action: 'select'; imageId: number; mode: VirtualMediaMode; writable: boolean }
  | { action: 'set_connected'; connected: boolean };

export type VirtualMediaResult = {
  applied: boolean;
  rejection?: string;
};

export function getMediaCatalog() {
  return http.get<{ images: MediaImage[] }>('/api/virtual-media/catalog');
}

export function submitVirtualMedia(operationId: string, command: VirtualMediaCommand) {
  return http.request<VirtualMediaResult>({
    method: 'post',
    url: '/api/virtual-media',
    data: { operationId, command },
    headers: { 'Content-Type': 'application/json' },
    timeout: 20000
  });
}
