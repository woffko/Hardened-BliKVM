import { useCallback, useEffect, useRef, useState } from 'react';
import { Button, Modal, Segmented, Switch } from 'antd';
import { useAtom, useAtomValue } from 'jotai';
import { useTranslation } from 'react-i18next';

import * as api from '@/api/transport.ts';
import { setTransportChoice } from '@/lib/localstorage.ts';
import { pagePort } from '@/lib/quic/messages.ts';
import { quicSession } from '@/lib/quic/session.ts';
import { leaseTransportAtom, quicStatusAtom, transportChoiceAtom } from '@/jotai/transport.ts';

/** The port the gateway listens on when nothing else is configured. */
const DEFAULT_QUIC_PORT = 443;
/** How long the gateway gets to reach the server after it was switched on. */
const LINK_WAIT_MS = 8000;
/** How often a gateway that is switched on but not yet there is asked for. */
const LINK_POLL_MS = 2000;

/**
 * Settings > Network > Transport: the device's QUIC switch, this browser's
 * choice between the standard transport and QUIC, and what is in use.
 */
export const Transport = ({ readOnly = false }: { readOnly?: boolean }) => {
  const { t } = useTranslation();
  const [choice, setChoice] = useAtom(transportChoiceAtom);
  const status = useAtomValue(quicStatusAtom);
  const leaseTransport = useAtomValue(leaseTransportAtom);
  const [quic, setQuic] = useState<api.QuicState | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [isSwitching, setIsSwitching] = useState(false);
  const [error, setError] = useState('');
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const rsp = await api.getTransport();
      if (mounted.current && rsp.code === 0) setQuic(rsp.data.quic);
      return rsp.code === 0 ? rsp.data.quic : null;
    } catch {
      return null;
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // What the device says is read again whenever the session changes state:
  // a session that opened or failed tells that something is different there.
  useEffect(() => {
    void refresh().then((current) => {
      if (!mounted.current) return;
      setIsLoading(false);
      setLoadFailed(current === null);
    });
  }, [refresh, status.state]);

  // Switched on, but the gateway is not on the server's link (it is starting,
  // or the server was restarted): look again until it is.
  const awaitsGateway = Boolean(quic?.enabled) && !quic?.connected;
  useEffect(() => {
    if (!awaitsGateway) return;
    const timer = setInterval(() => void refresh(), LINK_POLL_MS);
    return () => clearInterval(timer);
  }, [awaitsGateway, refresh]);

  async function switchDevice(enabled: boolean) {
    setIsSwitching(true);
    setError('');
    try {
      const rsp = enabled ? await api.enableQuic() : await api.disableQuic();
      if (rsp.code !== 0) throw new Error(rsp.msg);
      setQuic(rsp.data.quic);
      // The gateway takes a moment to start and to reach the server.
      const deadline = performance.now() + LINK_WAIT_MS;
      let current: api.QuicState | null = rsp.data.quic;
      while (enabled && mounted.current && !current?.connected && performance.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, 500));
        current = await refresh();
      }
      if (!enabled && choice === 'quic') choose('standard');
    } catch {
      if (mounted.current) setError(t('settings.network.transport.switchFailed'));
      void refresh();
    } finally {
      if (mounted.current) setIsSwitching(false);
    }
  }

  function requestSwitch(enabled: boolean) {
    if (readOnly || isSwitching) return;
    if (!enabled) {
      void switchDevice(false);
      return;
    }
    Modal.confirm({
      title: t('settings.network.transport.enableTitle'),
      content: t('settings.network.transport.enableDescription', {
        port: quic?.port ?? DEFAULT_QUIC_PORT
      }),
      okText: t('settings.network.transport.enableOk'),
      cancelText: t('settings.network.transport.cancel'),
      onOk: () => void switchDevice(true)
    });
  }

  function choose(next: 'standard' | 'quic') {
    setTransportChoice(next);
    setChoice(next);
  }

  // Why this browser cannot use QUIC right now, if it cannot.
  const hindrance =
    typeof WebTransport === 'undefined'
      ? 'unsupported'
      : !quic
        ? null
        : !quic.enabled
          ? 'off'
          : quic.blocked
            ? 'blocked'
            : quic.port === undefined
              ? 'unavailable'
              : quic.port !== pagePort(window.location)
                ? 'origin'
                : null;
  // Another attempt is offered unless this page could never have a session.
  const canRetry =
    choice === 'quic' &&
    status.state === 'failed' &&
    status.reason !== 'unsupported' &&
    status.reason !== 'origin';
  // QUIC can be left at any time; it can be chosen when nothing hinders it.
  const quicIsOffered = choice === 'quic' || (quic !== null && hindrance === null);

  const statusText =
    choice !== 'quic' || status.state === 'off'
      ? t('settings.network.transport.status.standard')
      : status.state === 'connecting'
        ? t('settings.network.transport.status.connecting')
        : status.state === 'connected'
          ? t('settings.network.transport.status.connected', {
              ms: status.roundTripMs === null ? '-' : status.roundTripMs.toFixed(1),
              lost: status.counters?.packetsLost ?? 0,
              keyFrames: status.counters?.keyFrames ?? 0
            })
          : t('settings.network.transport.fallback', {
              reason: t(`settings.network.transport.reasons.${status.reason}`)
            });

  return (
    <div className="flex flex-col space-y-4" data-testid="transport-settings">
      <div className="flex flex-col space-y-1">
        <span>{t('settings.network.transport.title')}</span>
        <span className="text-xs text-neutral-500">
          {t('settings.network.transport.description')}
        </span>
      </div>

      <div className="flex items-center justify-between gap-4">
        <div className="flex flex-col space-y-1">
          <span>{t('settings.network.transport.device')}</span>
          <span className="text-xs text-neutral-500">
            {t('settings.network.transport.deviceDescription', {
              port: quic?.port ?? DEFAULT_QUIC_PORT
            })}
          </span>
        </div>
        <Switch
          data-testid="transport-device-switch"
          checked={Boolean(quic?.enabled)}
          loading={isSwitching || isLoading}
          disabled={readOnly || quic === null || !quic.supported}
          onChange={requestSwitch}
        />
      </div>

      <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex flex-col space-y-1">
          <span>{t('settings.network.transport.browser')}</span>
          {hindrance && (
            <span className="text-xs text-neutral-500" data-testid="transport-hindrance">
              {t(`settings.network.transport.reasons.${hindrance}`)}
            </span>
          )}
        </div>
        <Segmented
          className="self-start sm:self-auto"
          data-testid="transport-choice"
          value={choice}
          options={[
            { value: 'standard', label: t('settings.network.transport.standard') },
            {
              value: 'quic',
              label: t('settings.network.transport.quic'),
              disabled: !quicIsOffered
            }
          ]}
          onChange={(value) => choose(value === 'quic' ? 'quic' : 'standard')}
        />
      </div>

      <div className="flex items-center justify-between gap-4">
        <span
          className={
            choice === 'quic' && status.state === 'failed'
              ? 'text-xs text-yellow-400/80'
              : 'text-xs text-neutral-500'
          }
          data-testid="transport-status"
          data-state={choice === 'quic' ? status.state : 'off'}
        >
          {statusText}
        </span>
        {canRetry && (
          <Button size="small" data-testid="transport-retry" onClick={() => quicSession.start()}>
            {t('settings.network.transport.retry')}
          </Button>
        )}
      </div>
      {choice === 'quic' && status.state === 'connected' && (
        <span
          className="text-xs text-neutral-500"
          data-testid="transport-input"
          data-input={leaseTransport ?? 'none'}
        >
          {t(
            leaseTransport === 'quic'
              ? 'settings.network.transport.input.quic'
              : leaseTransport === 'https'
                ? 'settings.network.transport.input.standard'
                : 'settings.network.transport.input.idle'
          )}
        </span>
      )}
      {(error || loadFailed) && (
        <span className="text-xs text-red-400" data-testid="transport-error">
          {error || t('settings.network.transport.loadFailed')}
        </span>
      )}
    </div>
  );
};
