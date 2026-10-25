import { http, type ApiResponse } from '@/lib/http.ts';

/** `GET /api/transport`: what the QUIC transport can do right now. */
export type QuicState = {
  /** This installation has a gateway's link configured. */
  supported: boolean;
  /** The board's switch: the gateway is enabled, and running. */
  enabled: boolean;
  active: boolean;
  /** The gateway is on the server's link: a session can be asked for. */
  connected: boolean;
  /** The firewall mode keeps the gateway's port closed. */
  blocked: boolean;
  port?: number;
};

export type TransportState = { quic: QuicState };

/** Why the server gives no session: nothing serves it, the firewall, the clock. */
export type QuicSessionRefusal = 'unavailable' | 'blocked' | 'clock';

export type QuicSessionGrant =
  | { granted: false; reason: QuicSessionRefusal }
  | {
      granted: true;
      port: number;
      certificateSha256: string;
      token: string;
      expiresInSeconds: number;
    };

export function getTransport(): Promise<ApiResponse<TransportState>> {
  return http.get('/api/transport');
}

export function enableQuic(): Promise<ApiResponse<TransportState>> {
  return http.post('/api/transport/quic/enable');
}

export function disableQuic(): Promise<ApiResponse<TransportState>> {
  return http.post('/api/transport/quic/disable');
}

/**
 * How the page's last session ended, from the page's side: what ended it
 * first, how long it had been open, how long since the gateway last answered
 * a ping, whether the page was hidden, and what the browser said. The server
 * only writes it in its journal (docs/design/quic-transport-v1.md).
 */
export type QuicSessionEnding = {
  ending:
    | 'no_pong'
    | 'stream_ended'
    | 'read_failed'
    | 'write_failed'
    | 'transport_closed'
    | 'gateway_closing';
  openSeconds: number;
  sincePongMs: number;
  hidden: boolean;
  detail?: string;
};

/**
 * A token for one QUIC session of this web session, with the page's clock,
 * and how the page's session before it ended.
 */
export function requestQuicSession(
  previous: QuicSessionEnding | null = null
): Promise<ApiResponse<QuicSessionGrant>> {
  return http.post('/api/transport/quic/session', {
    pageTime: Date.now(),
    ...(previous ? { previous } : {})
  });
}
