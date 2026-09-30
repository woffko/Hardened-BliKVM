import { http } from '@/lib/http.ts';

/** Administration features that stay off until switched on. */
export type AdminFeature = 'terminal' | 'scripts' | 'remote_download' | 'picoclaw';

export type AdminFeatures = {
  terminal: boolean;
  scripts: boolean;
  remoteDownload: boolean;
  picoclaw: boolean;
};

export function getAdminFeatures() {
  return http.get('/api/admin/features');
}

export function setAdminFeature(feature: AdminFeature, enabled: boolean) {
  return http.post('/api/admin/features', { feature, enabled });
}
