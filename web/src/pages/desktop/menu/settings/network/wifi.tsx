import { useCallback, useEffect, useRef, useState } from 'react';
import { Alert, Button, Input, Modal, Popconfirm, Spin, Switch, Tag } from 'antd';
import {
  LockIcon,
  RefreshCwIcon,
  WifiHighIcon,
  WifiIcon,
  WifiLowIcon,
  WifiOffIcon,
  WifiZeroIcon
} from 'lucide-react';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/network.ts';
import type { WifiNetwork, WifiProfile, WifiSecurity, WifiStatus } from '@/api/network.ts';

type Target = { ssid: string; security: WifiSecurity | null; hidden: boolean };

type Outcome = { code: number; data?: { applied?: boolean; rejection?: string | null } };

const JOINABLE: WifiSecurity[] = ['open', 'wpa2', 'wpa3', 'wpa2_wpa3'];

function SignalIcon({ signal }: { signal: number }) {
  if (signal >= 75) return <WifiIcon size={16} />;
  if (signal >= 50) return <WifiHighIcon size={16} />;
  if (signal >= 25) return <WifiLowIcon size={16} />;
  return <WifiZeroIcon size={16} />;
}

function validStatus(data: unknown): data is WifiStatus {
  const status = data as WifiStatus | undefined;
  return (
    typeof status?.available === 'boolean' &&
    typeof status.connected === 'boolean' &&
    typeof status.ssid === 'string' &&
    typeof status.radio === 'boolean'
  );
}

/** Wi-Fi on the board: the radio, the current network, the networks in range
 * and the saved ones. Passwords go to the board once and are never shown. */
export const Wifi = ({ readOnly = false }: { readOnly?: boolean }) => {
  const { t } = useTranslation();
  const text = useCallback(
    (key: string, options?: Record<string, unknown>) =>
      t(`settings.network.wifi.${key}`, options) as string,
    [t]
  );

  const [status, setStatus] = useState<WifiStatus | null>(null);
  const [networks, setNetworks] = useState<WifiNetwork[]>([]);
  const [profiles, setProfiles] = useState<WifiProfile[]>([]);
  const [scanning, setScanning] = useState(false);
  const [busy, setBusy] = useState('');
  const [error, setError] = useState('');
  const [target, setTarget] = useState<Target | null>(null);
  const [password, setPassword] = useState('');
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const loadStatus = useCallback(async () => {
    const rsp = await api.getWiFi();
    if (rsp.code !== 0 || !validStatus(rsp.data)) throw new Error('state');
    if (mounted.current) setStatus(rsp.data);
    return rsp.data;
  }, []);

  const loadSaved = useCallback(async () => {
    const rsp = await api.getSavedWifi();
    if (rsp.code === 0 && Array.isArray(rsp.data?.profiles) && mounted.current)
      setProfiles(rsp.data.profiles);
  }, []);

  const listNetworks = useCallback(async (rescan: boolean) => {
    const rsp = await api.scanWifi(rescan);
    if (rsp.code === 0 && Array.isArray(rsp.data?.networks) && mounted.current) {
      setNetworks(rsp.data.networks);
      return rsp.data.networks as WifiNetwork[];
    }
    return [];
  }, []);

  const scan = useCallback(
    async (rescan: boolean) => {
      setScanning(true);
      try {
        const found = await listNetworks(rescan);
        if (rescan || found.length === 0) {
          // A new scan reports its networks a few seconds later.
          if (!rescan) await listNetworks(true);
          await new Promise((resolve) => setTimeout(resolve, 4000));
          await listNetworks(false);
        }
      } finally {
        if (mounted.current) setScanning(false);
      }
    },
    [listNetworks]
  );

  const refreshAll = useCallback(
    async (withScan: boolean) => {
      try {
        const observed = await loadStatus();
        if (!observed.available) return;
        await loadSaved();
        if (observed.radio && withScan) await scan(false);
        else if (observed.radio) await listNetworks(false);
      } catch {
        if (mounted.current) setError(text('loadFailed'));
      }
    },
    [loadStatus, loadSaved, scan, listNetworks, text]
  );

  useEffect(() => {
    void refreshAll(true);
  }, [refreshAll]);

  async function run(label: string, action: () => Promise<Outcome>, failure = 'actionFailed') {
    if (readOnly || busy) return false;
    setBusy(label);
    setError('');
    let done = false;
    try {
      const rsp = await action();
      done = rsp.code === 0 && rsp.data?.applied === true;
      if (!done) setError(text(failure));
    } catch {
      setError(text(failure));
    }
    await refreshAll(false);
    if (mounted.current) setBusy('');
    return done;
  }

  function choose(network: WifiNetwork) {
    if (readOnly || busy || network.inUse) return;
    if (!JOINABLE.includes(network.security)) {
      setError(text('unsupportedSecurity'));
      return;
    }
    // A saved network is joined with its stored password.
    const saved = profiles.find((profile) => profile.ssid === network.ssid);
    if (saved) {
      void run(`use-${saved.id}`, () => api.wifiProfile(saved.id, 'connect'), 'failed');
      return;
    }
    setPassword('');
    setTarget({ ssid: network.ssid, security: network.security, hidden: false });
  }

  const needsPassword = !!target && !target.hidden && target.security !== 'open';
  const passwordValid =
    password === '' ? !needsPassword : password.length >= 8 && password.length <= 64;
  const targetValid =
    !!target && target.ssid.length > 0 && new TextEncoder().encode(target.ssid).length <= 32;

  async function connect() {
    if (!target || !targetValid || !passwordValid) return;
    const done = await run(
      'connect',
      () => api.connectWifi(target.ssid, password, target.hidden),
      'failed'
    );
    if (done && mounted.current) {
      setTarget(null);
      setPassword('');
    }
  }

  if (!status) {
    return error ? (
      <span role="alert" className="text-sm text-red-500">
        {error}
      </span>
    ) : null;
  }
  if (!status.available) return null;

  const radioBusy = busy === 'radio';
  const summary = !status.radio
    ? text('radioOff')
    : status.connected
      ? status.ssid
      : text('notConnected');

  return (
    <section className="flex flex-col space-y-4" data-testid="wifi-settings">
      <div className="flex items-center justify-between gap-3">
        <div className="flex min-w-0 flex-col space-y-1">
          <span>{text('title')}</span>
          <span className="truncate text-xs text-neutral-500">{summary}</span>
        </div>
        <Switch
          checked={status.radio}
          loading={radioBusy}
          disabled={readOnly || (busy !== '' && !radioBusy)}
          aria-label={text('radio')}
          onChange={(enabled) => void run('radio', () => api.setWifiRadio(enabled))}
        />
      </div>

      {error && (
        <Alert type="error" showIcon message={error} closable onClose={() => setError('')} />
      )}

      {status.radio && status.connected && (
        <div className="flex items-center justify-between gap-3 rounded-lg bg-neutral-800/50 p-3">
          <div className="flex min-w-0 items-center gap-3">
            <SignalIcon signal={status.signal ?? 0} />
            <div className="flex min-w-0 flex-col">
              <span className="truncate">{status.ssid}</span>
              <span className="truncate text-xs text-neutral-500">
                {[
                  text('connected'),
                  status.address,
                  status.security ? text(`security.${status.security}`) : ''
                ]
                  .filter(Boolean)
                  .join(' · ')}
              </span>
            </div>
          </div>
          <Popconfirm
            title={text('disconnect')}
            okText={text('confirmBtn')}
            cancelText={text('cancelBtn')}
            disabled={readOnly || busy !== ''}
            onConfirm={() => void run('disconnect', () => api.disconnectWifi())}
          >
            <Button
              size="small"
              loading={busy === 'disconnect'}
              disabled={readOnly || (busy !== '' && busy !== 'disconnect')}
            >
              {text('disconnectBtn')}
            </Button>
          </Popconfirm>
        </div>
      )}

      {status.radio && (
        <div className="flex flex-col space-y-2">
          <div className="flex items-center justify-between">
            <span className="text-sm text-neutral-400">{text('available')}</span>
            <Button
              size="small"
              type="text"
              icon={<RefreshCwIcon size={14} />}
              loading={scanning}
              disabled={busy !== ''}
              onClick={() => void scan(true)}
            >
              {text('refresh')}
            </Button>
          </div>
          {networks.length === 0 ? (
            <div className="flex items-center gap-2 px-1 text-xs text-neutral-500">
              {scanning ? <Spin size="small" /> : null}
              {scanning ? text('scanning') : text('noNetworks')}
            </div>
          ) : (
            <div className="flex flex-col divide-y divide-neutral-700/60 overflow-hidden rounded-lg bg-neutral-800/30">
              {networks.map((network) => {
                const joinable = JOINABLE.includes(network.security);
                return (
                  <button
                    key={network.ssid}
                    type="button"
                    data-testid="wifi-network"
                    className="flex min-h-[44px] w-full cursor-pointer items-center justify-between gap-3 border-0 bg-transparent px-3 py-2 text-left text-neutral-200 hover:bg-neutral-700/40 disabled:cursor-default disabled:hover:bg-transparent"
                    disabled={readOnly || busy !== '' || network.inUse}
                    onClick={() => choose(network)}
                  >
                    <span className="flex min-w-0 items-center gap-3">
                      <span className="shrink-0 text-neutral-300">
                        <SignalIcon signal={network.signal} />
                      </span>
                      <span className="truncate">{network.ssid}</span>
                      {network.security !== 'open' && (
                        <LockIcon
                          size={12}
                          className="shrink-0 text-neutral-500"
                          aria-label={text(`security.${network.security}`)}
                        />
                      )}
                    </span>
                    <span className="flex shrink-0 items-center gap-2 text-xs text-neutral-500">
                      {network.inUse && (
                        <Tag color="green" className="m-0">
                          {text('connected')}
                        </Tag>
                      )}
                      {!joinable && (
                        <Tag className="m-0">{text(`security.${network.security}`)}</Tag>
                      )}
                      {busy === `use-${profiles.find((p) => p.ssid === network.ssid)?.id}` ? (
                        <Spin size="small" />
                      ) : (
                        `${network.signal}%`
                      )}
                    </span>
                  </button>
                );
              })}
            </div>
          )}
          <Button
            type="link"
            size="small"
            className="self-start px-0"
            disabled={readOnly || busy !== ''}
            onClick={() => {
              setPassword('');
              setTarget({ ssid: '', security: null, hidden: true });
            }}
          >
            {text('other')}
          </Button>
        </div>
      )}

      {!status.radio && (
        <div className="flex items-center gap-2 text-sm text-neutral-500">
          <WifiOffIcon size={16} />
          {text('radioOffDesc')}
        </div>
      )}

      {profiles.length > 0 && (
        <div className="flex flex-col space-y-2">
          <span className="text-sm text-neutral-400">{text('saved')}</span>
          <div className="flex flex-col divide-y divide-neutral-700/60 overflow-hidden rounded-lg bg-neutral-800/30">
            {profiles.map((profile) => (
              <div
                key={profile.id}
                data-testid="wifi-profile"
                className="flex flex-wrap items-center justify-between gap-2 px-3 py-2"
              >
                <span className="flex min-w-0 items-center gap-2">
                  <span className="truncate">{profile.ssid || text('unnamed')}</span>
                  {profile.active && (
                    <Tag color="green" className="m-0">
                      {text('connected')}
                    </Tag>
                  )}
                </span>
                <span className="flex shrink-0 items-center gap-2">
                  <span className="text-xs text-neutral-500">{text('autoconnect')}</span>
                  <Switch
                    size="small"
                    checked={profile.autoconnect}
                    loading={busy === `auto-${profile.id}`}
                    disabled={readOnly || busy !== ''}
                    aria-label={`${text('autoconnect')}: ${profile.ssid}`}
                    onChange={(enabled) =>
                      void run(`auto-${profile.id}`, () =>
                        api.wifiProfile(profile.id, enabled ? 'autoconnect_on' : 'autoconnect_off')
                      )
                    }
                  />
                  {!profile.active && (
                    <Button
                      size="small"
                      disabled={readOnly || busy !== '' || !status.radio}
                      loading={busy === `use-${profile.id}`}
                      onClick={() =>
                        void run(
                          `use-${profile.id}`,
                          () => api.wifiProfile(profile.id, 'connect'),
                          'failed'
                        )
                      }
                    >
                      {text('joinBtn')}
                    </Button>
                  )}
                  <Popconfirm
                    title={text('forgetConfirm', { ssid: profile.ssid || text('unnamed') })}
                    okText={text('confirmBtn')}
                    cancelText={text('cancelBtn')}
                    disabled={readOnly || busy !== ''}
                    onConfirm={() =>
                      void run(`forget-${profile.id}`, () => api.wifiProfile(profile.id, 'forget'))
                    }
                  >
                    <Button
                      size="small"
                      danger
                      loading={busy === `forget-${profile.id}`}
                      disabled={readOnly || (busy !== '' && busy !== `forget-${profile.id}`)}
                    >
                      {text('forget')}
                    </Button>
                  </Popconfirm>
                </span>
              </div>
            ))}
          </div>
        </div>
      )}

      <Modal
        open={target !== null}
        centered
        closable={false}
        title={target?.hidden ? text('other') : text('connect')}
        okText={text('joinBtn')}
        cancelText={text('cancelBtn')}
        confirmLoading={busy === 'connect'}
        okButtonProps={{ disabled: readOnly || !targetValid || !passwordValid }}
        cancelButtonProps={{ disabled: busy === 'connect' }}
        onOk={() => void connect()}
        onCancel={() => busy !== 'connect' && setTarget(null)}
        destroyOnHidden
      >
        {target && (
          <div className="flex flex-col space-y-3">
            <span className="text-sm text-neutral-400">
              {target.hidden
                ? text('otherDesc')
                : needsPassword
                  ? text('connectDesc2')
                  : text('openDesc')}
            </span>
            {target.hidden ? (
              <Input
                placeholder={text('ssid')}
                aria-label={text('ssid')}
                maxLength={32}
                value={target.ssid}
                disabled={busy === 'connect'}
                onChange={(event) => setTarget({ ...target, ssid: event.target.value })}
              />
            ) : (
              <span className="font-medium">{target.ssid}</span>
            )}
            {(needsPassword || target.hidden) && (
              <Input.Password
                placeholder={target.hidden ? text('passwordOptional') : text('password')}
                aria-label={text('password')}
                maxLength={64}
                autoComplete="off"
                value={password}
                disabled={busy === 'connect'}
                onChange={(event) => setPassword(event.target.value)}
                onPressEnter={() => void connect()}
              />
            )}
            {busy === 'connect' && (
              <span className="text-xs text-neutral-500">{text('connecting')}</span>
            )}
          </div>
        )}
      </Modal>
    </section>
  );
};
