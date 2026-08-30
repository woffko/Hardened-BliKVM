import { http, HttpResponse } from 'msw';
import { setupWorker } from 'msw/browser';

export const handlers = [
  http.get('/api/capabilities', () => {
    return HttpResponse.json({
      code: 0,
      msg: 'success',
      data: {
        schemaVersion: 1,
        product: {
          name: 'Hardened BliKVM',
          shortName: 'Hardened-BliKVM',
          tagline: 'Hardened with Rust'
        },
        platform: {
          id: 'bli-h616-pikvm',
          board: 'BliKVM v4 H616',
          architecture: 'aarch64',
          operatingSystem: 'PiKVM/KVMD on Armbian'
        },
        video: {
          status: { availability: 'unavailable', evidence: 'runtime_verified' },
          transports: ['mjpeg'],
          preferredTransport: 'mjpeg',
          maxWidth: null,
          maxHeight: null,
          audio: { availability: 'unavailable', evidence: 'none' }
        },
        hid: {
          status: { availability: 'unavailable', evidence: 'runtime_verified' },
          modes: ['keyboard', 'absolute_mouse', 'relative_mouse']
        },
        power: {
          status: { availability: 'unavailable', evidence: 'runtime_verified' },
          actions: ['power_short', 'power_long', 'reset'],
          ledState: { availability: 'unavailable', evidence: 'runtime_verified' }
        },
        virtualMedia: {
          status: { availability: 'unavailable', evidence: 'runtime_verified' },
          modes: ['cd_rom', 'removable_disk'],
          resumableUpload: { availability: 'unavailable', evidence: 'none' },
          remoteDownload: { availability: 'unavailable', evidence: 'none' }
        },
        board: { features: {} },
        system: {
          status: { availability: 'read_only', evidence: 'user_reported' },
          readOnlyRootfs: true,
          maintenanceWriteMode: true,
          signedApplicationUpdates: { availability: 'unavailable', evidence: 'none' },
          signedSystemUpdates: { availability: 'unavailable', evidence: 'none' },
          automaticRollback: { availability: 'unavailable', evidence: 'none' }
        }
      }
    });
  }),
  http.post('/api/auth/login', () => {
    return HttpResponse.json({
      code: 0,
      data: {
        token: 'mocked_token'
      }
    });
  })
];
export const worker = setupWorker(...handlers);
