import type { CapabilityManifest } from '@/types/capabilities.ts';

import { http } from '@/lib/http.ts';

export function getCapabilities() {
  return http.get<CapabilityManifest>('/api/capabilities');
}
