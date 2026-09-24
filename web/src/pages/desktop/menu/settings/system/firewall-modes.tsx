import { useCallback, useEffect, useState } from 'react';
import { Button, Popconfirm } from 'antd';
import { useAtomValue } from 'jotai';

import { changeMode, confirmMode, getModeStatus } from '@/api/system-firewall.ts';
import type { FirewallMode } from '@/api/system-firewall.ts';
import { boardFeatureIsWritable } from '@/lib/capabilities.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';

const modes: FirewallMode[] = ['baseline', 'moderate', 'restricted', 'paranoid'];
const operationPattern = /^(?!0{32}$)[0-9a-f]{32}$/;
type Status = {
  mode: FirewallMode;
  pendingOperationId: string | null;
  confirmationDeadlineUnixMs: string | null;
  recoveryRequired: boolean;
  configurationReady: boolean;
};

function decode(value: unknown): Status {
  if (typeof value !== 'object' || value === null) throw new Error('Invalid firewall status');
  const data = value as Record<string, unknown>;
  if (
    !modes.includes(data.mode as FirewallMode) ||
    typeof data.recoveryRequired !== 'boolean' ||
    typeof data.configurationReady !== 'boolean'
  )
    throw new Error('Invalid firewall status');
  if (data.pendingOperationId === null) {
    if (data.confirmationDeadlineUnixMs !== null || data.recoveryRequired)
      throw new Error('Invalid pending firewall status');
  } else if (
    typeof data.pendingOperationId !== 'string' ||
    !operationPattern.test(data.pendingOperationId) ||
    typeof data.confirmationDeadlineUnixMs !== 'string' ||
    !/^[1-9][0-9]{0,15}$/.test(data.confirmationDeadlineUnixMs) ||
    !Number.isSafeInteger(Number(data.confirmationDeadlineUnixMs))
  )
    throw new Error('Invalid pending firewall status');
  return value as Status;
}

export const FirewallModes = () => {
  const manifest = useAtomValue(capabilitiesAtom);
  const writable = boardFeatureIsWritable(manifest, 'system_firewall_modes');
  const [status, setStatus] = useState<Status | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [now, setNow] = useState(Date.now());
  const load = useCallback(async () => {
    const response = await getModeStatus();
    if (response.code !== 0) throw new Error('Unable to read firewall modes');
    const observed = decode(response.data);
    setStatus(observed);
    setNow(Date.now());
    return observed;
  }, []);
  useEffect(() => {
    void load().catch(() => setError('Unable to read firewall modes'));
  }, [load]);
  useEffect(() => {
    if (!status?.pendingOperationId) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        await load();
      } catch {
        if (!cancelled) setError('Unable to refresh pending firewall state');
      }
      if (!cancelled) timer = setTimeout(() => void poll(), 1000);
    };
    timer = setTimeout(() => void poll(), 1000);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [status?.pendingOperationId, load]);

  async function apply(mode: FirewallMode) {
    if (
      !writable ||
      busy ||
      !status?.configurationReady ||
      status.pendingOperationId ||
      status.recoveryRequired
    )
      return;
    setBusy(true);
    setError('');
    try {
      const response = await changeMode(mode);
      if (
        response.code !== 0 ||
        response.data?.applied !== true ||
        response.data?.rejection != null ||
        typeof response.data?.operationId !== 'string' ||
        !operationPattern.test(response.data.operationId)
      )
        throw new Error(response.data?.rejection || 'Firewall mode change refused');
      const observed = await load();
      const pending = mode === 'restricted' || mode === 'paranoid';
      if (
        observed.mode !== mode ||
        observed.recoveryRequired ||
        (pending
          ? observed.pendingOperationId !== response.data.operationId
          : observed.pendingOperationId !== null)
      )
        throw new Error('Firewall mode read-back mismatch');
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Firewall mode change failed');
    } finally {
      setBusy(false);
    }
  }

  async function confirm() {
    if (
      !writable ||
      busy ||
      !status?.pendingOperationId ||
      !status.configurationReady ||
      status.recoveryRequired
    )
      return;
    const expected = status.mode;
    const operation = status.pendingOperationId;
    setBusy(true);
    setError('');
    try {
      const response = await confirmMode(operation);
      if (
        response.code !== 0 ||
        response.data?.applied !== true ||
        response.data?.rejection != null ||
        response.data?.operationId !== operation
      )
        throw new Error(response.data?.rejection || 'Firewall confirmation refused');
      const observed = await load();
      if (
        observed.pendingOperationId !== null ||
        observed.mode !== expected ||
        observed.recoveryRequired
      )
        throw new Error('Firewall confirmation read-back mismatch');
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Firewall confirmation failed');
    } finally {
      setBusy(false);
    }
  }
  const remaining = status?.confirmationDeadlineUnixMs
    ? Math.max(0, Math.ceil((Number(status.confirmationDeadlineUnixMs) - now) / 1000))
    : 0;
  return (
    <section className="flex flex-col gap-4">
      <div className="text-base">Firewall modes</div>
      {error && (
        <div role="alert" className="text-red-400">
          {error}
        </div>
      )}
      {status && (
        <>
          <div>Current mode: {status.mode}</div>
          {!status.configurationReady && (
            <div role="alert">Firewall configuration requires reconciliation.</div>
          )}
          {status.recoveryRequired && <div role="alert">Firewall recovery is pending.</div>}
          <div className="flex flex-wrap gap-3">
            {modes.map((mode) => (
              <Popconfirm
                key={mode}
                title={`Apply ${mode} firewall mode?`}
                okText="Apply"
                cancelText="Cancel"
                onConfirm={() => apply(mode)}
                disabled={
                  !writable ||
                  busy ||
                  !status.configurationReady ||
                  Boolean(status.pendingOperationId) ||
                  status.recoveryRequired ||
                  mode === status.mode
                }
              >
                <Button
                  disabled={
                    !writable ||
                    busy ||
                    !status.configurationReady ||
                    Boolean(status.pendingOperationId) ||
                    status.recoveryRequired ||
                    mode === status.mode
                  }
                >
                  {mode}
                </Button>
              </Popconfirm>
            ))}
          </div>
          {status.pendingOperationId && (
            <div className="flex flex-col gap-2">
              <span>
                Confirm connectivity within {remaining} seconds or the owner will roll back.
              </span>
              <Button
                onClick={() => void confirm()}
                disabled={
                  !writable ||
                  busy ||
                  !status.configurationReady ||
                  status.recoveryRequired ||
                  remaining === 0
                }
              >
                Confirm firewall connectivity
              </Button>
            </div>
          )}
        </>
      )}
    </section>
  );
};
