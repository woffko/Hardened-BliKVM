import type { DiagnosticsView } from '@/types/diagnostics.ts';

import { http } from '@/lib/http.ts';

export function getDiagnostics() {
  return http.get<DiagnosticsView>('/api/diagnostics');
}
