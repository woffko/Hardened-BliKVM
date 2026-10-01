import { useEffect } from 'react';
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
 * Keeps this page's QUIC session open while QUIC is this browser's choice
 * and the device offers it. A session that fails is said once, with its
 * reason; the standard transport goes on underneath.
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
  useEffect(() => {
    if (!failure) {
      api.destroy(NOTICE_KEY);
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
  }, [api, failure, setVideoGivenUp, t]);

  return <>{contextHolder}</>;
};
