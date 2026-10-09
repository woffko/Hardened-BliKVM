import { useEffect, useRef } from 'react';
import { Button, notification } from 'antd';
import { useAtomValue, useSetAtom } from 'jotai';
import { useTranslation } from 'react-i18next';

import { supportsBoardFeature } from '@/lib/capabilities.ts';
import { quicSession, type QuicFailure } from '@/lib/quic/session.ts';
import { capabilitiesAtom } from '@/jotai/capabilities.ts';
import { quicStatusAtom, quicVideoGivenUpAtom, transportChoiceAtom } from '@/jotai/transport.ts';

const NOTICE_KEY = 'quic-transport';
/** Failures another attempt may cure: the notice offers one. */
const RETRYABLE: ReadonlySet<QuicFailure> = new Set(['timeout', 'lost', 'unavailable']);
/**
 * Failures the page also tries again by itself, the standard transport
 * serving meanwhile: once the page is looked at again (a phone's screen
 * that was off), and every RETRY_MS while it is. A refusal for too many
 * sessions is one of them: a session the page lost on a bad path counts
 * until the gateway lets it go, 30 s after its last packet.
 */
const RETRIED: ReadonlySet<QuicFailure> = new Set(['timeout', 'lost', 'unavailable', 'sessions']);
const RETRY_MS = 35000;
/**
 * The picture's reasons for leaving the session that a session opened anew
 * may cure: the session's own failures, and a picture that stalled.
 */
const CURED_BY_A_SESSION: ReadonlySet<string> = new Set([...RETRIED, 'stalled']);

/**
 * Keeps this page's QUIC session open while QUIC is this browser's choice
 * and the device offers it. A session that fails is said once, with its
 * reason; the standard transport goes on underneath, and where waiting may
 * cure the failure the page tries again by itself and takes the picture
 * back over the session once it is open.
 */
export const QuicTransport = () => {
  const { t } = useTranslation();
  const [api, contextHolder] = notification.useNotification();
  const capabilities = useAtomValue(capabilitiesAtom);
  const choice = useAtomValue(transportChoiceAtom);
  const status = useAtomValue(quicStatusAtom);
  const setVideoGivenUp = useSetAtom(quicVideoGivenUpAtom);
  const offered = supportsBoardFeature(capabilities, 'network_quic');

  useEffect(() => {
    if (choice !== 'quic' || !offered) return;
    quicSession.start();
    return () => quicSession.stop();
  }, [choice, offered]);

  const failure = status.state === 'failed' ? status.reason : null;
  // Set while an attempt the page made by itself is under way: its failure
  // was said already.
  const retrying = useRef(false);
  useEffect(() => {
    if (choice !== 'quic' || !offered || !failure || !RETRIED.has(failure)) return;
    const again = () => {
      if (document.hidden) return;
      retrying.current = true;
      quicSession.start();
    };
    const timer = window.setInterval(again, RETRY_MS);
    document.addEventListener('visibilitychange', again);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', again);
    };
  }, [choice, offered, failure]);

  // The session is open again: the picture comes back over it where it left
  // for a reason the new session cures.
  useEffect(() => {
    if (status.state !== 'connected') return;
    setVideoGivenUp((reason) =>
      reason !== null && CURED_BY_A_SESSION.has(reason) ? null : reason
    );
  }, [status.state, setVideoGivenUp]);

  useEffect(() => {
    if (status.state === 'connected' || status.state === 'off') {
      retrying.current = false;
      api.destroy(NOTICE_KEY);
      return;
    }
    // While an attempt is under way the notice stays as it is.
    if (!failure) return;
    if (retrying.current) {
      retrying.current = false;
      return;
    }
    api.warning({
      key: NOTICE_KEY,
      message: t('settings.network.transport.fallbackTitle'),
      description: (
        <span data-testid="transport-notice" data-reason={failure}>
          {t('settings.network.transport.fallback', {
            reason: t(`settings.network.transport.reasons.${failure}`)
          })}
        </span>
      ),
      placement: 'topRight',
      duration: 8,
      btn: RETRYABLE.has(failure) ? (
        <Button
          size="small"
          data-testid="transport-notice-retry"
          onClick={() => {
            // Another attempt for the session, and for the picture over it.
            setVideoGivenUp(null);
            quicSession.start();
          }}
        >
          {t('settings.network.transport.retry')}
        </Button>
      ) : undefined
    });
  }, [api, status.state, failure, setVideoGivenUp, t]);

  return <>{contextHolder}</>;
};
