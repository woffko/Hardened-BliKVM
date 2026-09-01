import type { DiagnosticsView } from '@/types/diagnostics.ts';

import { http } from '@/lib/http.ts';

export function getDiagnostics(signal?: AbortSignal) {
  return http.request<DiagnosticsView>({
    method: 'get',
    url: '/api/diagnostics',
    signal
  });
}
