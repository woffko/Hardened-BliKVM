import { useCallback, useEffect, useState } from 'react';
import { Button, Popconfirm } from 'antd';
import { useAtomValue } from 'jotai';

import { getStatus, setNativeMode } from '@/api/system-firewall.ts';
import { boardFeatureIsWritable } from '@/lib/capabilities.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';

type NativeStatus = {
  backendAvailable: boolean;
  baselineActive: boolean;
  mode: string;
  message: string;
  nftInstalled: boolean;
  nftDetail: string;
  iptablesInstalled: boolean;
  ip6tablesInstalled: boolean;
  rulesNft: string;
};

function decode(value: unknown): NativeStatus {
  if (typeof value !== 'object' || value === null) throw new Error('Invalid firewall response');
  const data = value as Record<string, unknown>;
  if (
    ![
      'backendAvailable',
      'baselineActive',
      'nftInstalled',
      'iptablesInstalled',
      'ip6tablesInstalled'
    ].every((key) => typeof data[key] === 'boolean') ||
    !['mode', 'message', 'nftDetail', 'rulesNft'].every((key) => typeof data[key] === 'string')
  )
    throw new Error('Invalid firewall response');
  return value as NativeStatus;
}

/** The reviewed baseline toggle, for boards whose owner runs no modes. */
export const NativeFirewall = () => {
  const manifest = useAtomValue(capabilitiesAtom);
  const writable = boardFeatureIsWritable(manifest, 'system_firewall');
  const [status, setStatus] = useState<NativeStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const load = useCallback(async () => {
    const response = await getStatus();
    if (response.code !== 0) throw new Error('Unable to read firewall status');
    const observed = decode(response.data);
    setStatus(observed);
    return observed;
  }, []);
  useEffect(() => {
    void load().catch(() => setError('Unable to read firewall status'));
  }, [load]);

  async function apply(mode: 'baseline' | 'disabled') {
    if (!writable || busy || !status?.backendAvailable) return;
    setBusy(true);
    setError('');
    try {
      const response = await setNativeMode(mode);
      if (
        response.code !== 0 ||
        response.data?.applied !== true ||
        response.data?.rejection != null
      )
        throw new Error(response.data?.rejection || 'Firewall change was refused');
      const observed = await load();
      if (observed.mode !== mode || observed.baselineActive !== (mode === 'baseline'))
        throw new Error('Firewall read-back did not match the requested state');
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Unable to update firewall');
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="flex flex-col gap-4">
      <div className="text-base">Firewall</div>
      {error && (
        <div role="alert" className="text-red-400">
          {error}
        </div>
      )}
      {status && (
        <>
          <div>Managed rules: {status.baselineActive ? 'active' : 'inactive'}</div>
          <div>Backend: {status.backendAvailable ? 'available' : 'unavailable'}</div>
          <div>{status.message}</div>
          <div className="flex flex-wrap gap-3">
            {(['baseline', 'disabled'] as const).map((mode) => (
              <Popconfirm
                key={mode}
                title={
                  mode === 'baseline'
                    ? 'Enable managed firewall rules?'
                    : 'Disable managed firewall rules?'
                }
                onConfirm={() => apply(mode)}
                okText="Apply"
                cancelText="Cancel"
                disabled={!writable || busy || !status.backendAvailable || status.mode === mode}
              >
                <Button
                  disabled={!writable || busy || !status.backendAvailable || status.mode === mode}
                >
                  {mode === 'baseline' ? 'Enable managed rules' : 'Disable managed rules'}
                </Button>
              </Popconfirm>
            ))}
          </div>
          <div className="text-sm text-neutral-400">
            Reference moderate, restricted and paranoid modes are not implemented by this backend
            yet.
          </div>
          <div>
            nftables: {status.nftInstalled ? status.nftDetail || 'installed' : 'not installed'}
          </div>
          <pre className="max-h-80 overflow-auto whitespace-pre-wrap break-all text-xs">
            {status.rulesNft || 'No rules reported'}
          </pre>
        </>
      )}
    </section>
  );
};
