/**
 * The HID lease over the QUIC session's input stream: the requests the page
 * sends to `/api/hid/lease`, answered by the same code on the server.
 */
import type { LeaseGrant } from '@/api/restricted-control.ts';
import { getTransport } from '@/api/transport.ts';
import type { LeaseTransport } from '@/lib/hid-lease.ts';
import type { ApiResponse } from '@/lib/http.ts';

import { quicSession } from './session.ts';

async function send(request: object, signal?: AbortSignal): Promise<ApiResponse<LeaseGrant>> {
  const answer = await quicSession.input(request, signal);
  if (answer.status === 401) {
    // The web session has ended. The server says so to a request over HTTPS
    // too, and the page then asks for a login.
    void getTransport().catch(() => undefined);
  }
  // As over HTTPS, where any other status fails the request.
  if (answer.status !== 200) {
    throw new Error(`the lease request was answered with status ${answer.status}`);
  }
  return answer.body as ApiResponse<LeaseGrant>;
}

export const quicLease: LeaseTransport = {
  acquire: (operationId, ttlSeconds, signal) =>
    send({ action: 'acquire', operationId, ttlSeconds }, signal),
  renew: (operationId, leaseId, ttlSeconds, signal) =>
    send({ action: 'renew', operationId, leaseId, ttlSeconds }, signal),
  report: (operationId, leaseId, report, signal) =>
    send({ action: 'report', operationId, leaseId, report }, signal),
  release: (operationId, leaseId, signal) =>
    send({ action: 'release', operationId, leaseId }, signal)
};
