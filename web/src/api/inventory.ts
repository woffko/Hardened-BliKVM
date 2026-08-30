import type { RuntimeInventory } from '@/types/inventory.ts';

import { http } from '@/lib/http.ts';

export function getRuntimeInventory() {
  return http.get<RuntimeInventory>('/api/inventory');
}
