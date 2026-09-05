import type { ApplicationUpdateStatus } from '@/types/application-update.ts';

import { http } from '@/lib/http.ts';

export function getApplicationUpdateStatus(signal?: AbortSignal) {
  return http.request<ApplicationUpdateStatus>({
    method: 'get',
    url: '/api/application-update/status',
    signal,
    timeout: 8000
  });
}
