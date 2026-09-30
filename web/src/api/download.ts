import { http } from '@/lib/http.ts';

// Download image
export function downloadImage(file?: string) {
  const data = {
    file: file ? file : ''
  };
  return http.post('/api/download/image', data);
}

export function statusImage() {
  return http.get('/api/download/image/status');
}

export function imageEnabled() {
  return http.get('/api/download/image/enabled');
}

// The board's refusals for a file, asked before the file is sent.
export function checkUpload(name: string, size: number) {
  return http.post('/api/download/file/check', { name, size });
}

export function getRemoteImageDownloadEnabled() {
  return http.get('/api/download/image/remote/enabled');
}

export function setRemoteImageDownloadEnabled(enabled: boolean) {
  return http.post('/api/download/image/remote/enabled', { enabled });
}
