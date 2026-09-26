import { http } from '@/lib/http.ts';

export type RawSystemUpdateState = {
  phase: string;
  version: string;
  sequence: number;
  message: string;
  boot_attempts: number;
  image_size: number;
  minimum_sequence: number;
};

export type SystemActionOutcome = { applied: boolean; rejection?: string };

export function getRawSystemUpdatesEnabled() {
  return http.request<{ enabled: boolean }>({
    method: 'get',
    url: '/api/system-update/raw-enabled',
    timeout: 8000
  });
}

export function setRawSystemUpdatesEnabled(enabled: boolean) {
  return http.request<{ enabled: boolean }>({
    method: 'post',
    url: '/api/system-update/raw-enabled',
    data: { enabled },
    timeout: 15000
  });
}

export function getRawSystemUpdateStatus() {
  return http.request<RawSystemUpdateState>({
    method: 'get',
    url: '/api/system-update/status',
    timeout: 8000
  });
}

export function uploadSystemBundle(
  file: File,
  signal: AbortSignal,
  progress: (loaded: number) => void
) {
  return http.request<RawSystemUpdateState>({
    method: 'post',
    url: '/api/system-update/upload',
    data: file,
    headers: { 'Content-Type': 'application/octet-stream' },
    signal,
    // Verifying the bundle and archiving the device state follow the transfer.
    timeout: 0,
    onUploadProgress: (event) => progress(event.loaded)
  });
}

export function installSystemUpdate() {
  return http.request<SystemActionOutcome>({
    method: 'post',
    url: '/api/system-update/install',
    data: { confirm: true },
    timeout: 15000
  });
}

export function confirmSystemUpdate() {
  return http.request<RawSystemUpdateState>({
    method: 'post',
    url: '/api/system-update/confirm',
    timeout: 15000
  });
}

export function rollbackSystemUpdate() {
  return http.request<SystemActionOutcome>({
    method: 'post',
    url: '/api/system-update/rollback',
    data: { confirm: true },
    timeout: 15000
  });
}

export function cancelSystemUpdate() {
  return http.request<RawSystemUpdateState>({
    method: 'post',
    url: '/api/system-update/cancel',
    timeout: 15000
  });
}
