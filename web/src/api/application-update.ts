import type {
  ApplicationUpdateStatus,
  ApplicationUploadReceipt
} from '@/types/application-update.ts';

import { http } from '@/lib/http.ts';

export function getApplicationUpdateStatus(signal?: AbortSignal) {
  return http.request<ApplicationUpdateStatus>({
    method: 'get',
    url: '/api/application-update/status',
    signal,
    timeout: 8000
  });
}

export function uploadApplicationArchive(
  file: File,
  signal: AbortSignal,
  progress: (loaded: number) => void
) {
  return http.request<ApplicationUploadReceipt>({
    method: 'post',
    url: '/api/application-update/upload',
    data: file,
    headers: { 'Content-Type': 'application/octet-stream' },
    // The browser supplies Content-Length from this exact File. Never send
    // multipart, a client pathname, an upstream URL or an activation flag.
    signal,
    timeout: 185000,
    onUploadProgress: (event) => progress(event.loaded)
  });
}
