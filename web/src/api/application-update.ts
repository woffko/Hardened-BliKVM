import type {
  ApplicationActivationReceipt,
  ApplicationUpdateStatus,
  ApplicationUploadReceipt
} from '@/types/application-update.ts';

import { http } from '@/lib/http.ts';

export function activateApplication(
  slotId: string,
  expectedGeneration: string,
  signal: AbortSignal
) {
  return http.request<ApplicationActivationReceipt>({
    method: 'post',
    url: '/api/application-update/activate',
    data: { slotId, expectedGeneration },
    headers: { 'Content-Type': 'application/json' },
    signal,
    timeout: 15000
  });
}

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
