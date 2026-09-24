import { useEffect, useState } from 'react';
import { Divider } from 'antd';

import { getStatus } from '@/api/extensions/tailscale.ts';

type NativeStatus = {
  installed: boolean;
  running: boolean;
  enabled: boolean;
  loginName: string;
  deviceIp: string;
  deviceName: string;
  message: string;
};

function isNativeStatus(value: unknown): value is NativeStatus {
  if (typeof value !== 'object' || value === null) return false;
  const data = value as Record<string, unknown>;
  return (
    ['installed', 'running', 'enabled'].every((key) => typeof data[key] === 'boolean') &&
    ['loginName', 'deviceIp', 'deviceName', 'message'].every((key) => typeof data[key] === 'string')
  );
}

/** The native owner currently offers observation only; all actions are refused. */
export const NativeTailscale = () => {
  const [status, setStatus] = useState<NativeStatus | null>(null);
  const [error, setError] = useState('');
  useEffect(() => {
    let cancelled = false;
    getStatus()
      .then((response) => {
        if (cancelled) return;
        if (response.code !== 0 || !isNativeStatus(response.data))
          throw new Error('Unable to read Tailscale status');
        setStatus(response.data);
      })
      .catch(() => {
        if (!cancelled) setError('Unable to read Tailscale status');
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <>
      <div className="text-base">Tailscale</div>
      <Divider className="opacity-50" />
      {error ? (
        <div role="alert" className="text-red-400">
          {error}
        </div>
      ) : status ? (
        <>
          <dl className="grid grid-cols-2 gap-3">
            <dt>Installed</dt>
            <dd>{status.installed ? 'Yes' : 'No'}</dd>
            <dt>Running</dt>
            <dd>{status.running ? 'Yes' : 'No'}</dd>
            <dt>Enabled</dt>
            <dd>{status.enabled ? 'Yes' : 'No'}</dd>
            <dt>Login</dt>
            <dd>{status.loginName || '—'}</dd>
            <dt>Device</dt>
            <dd>{status.deviceName || '—'}</dd>
            <dt>IP address</dt>
            <dd>{status.deviceIp || '—'}</dd>
          </dl>
          {status.message && <p className="mt-4 text-sm text-neutral-400">{status.message}</p>}
          <p className="mt-4 text-sm text-neutral-400">
            Read-only status. Tailscale actions are unavailable on this backend.
          </p>
        </>
      ) : (
        <div>Loading Tailscale status…</div>
      )}
    </>
  );
};
