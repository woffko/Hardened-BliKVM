import { http } from '@/lib/http.ts';

export type FocusChallenge = {
  challenge: string;
  expiresInSeconds: number;
};

export type FocusConfirmation = {
  attested: boolean;
  rejection?: string;
};

export type LeaseGrant = {
  applied: boolean;
  leaseId?: string;
  ttlSeconds?: number;
  rejection?: string;
};

export type PowerResult = {
  applied: boolean;
  rejection?: string;
};

export type HidReport =
  | { mode: 'keyboard'; data: number[] }
  | { mode: 'relative_mouse'; data: number[] }
  | { mode: 'absolute_mouse'; data: number[] };

export function getFocusChallenge(signal?: AbortSignal) {
  return http.request<FocusChallenge>({
    method: 'get',
    url: '/api/hid/focus',
    signal,
    timeout: 8000
  });
}

export function confirmFocus(
  operationId: string,
  challenge: string,
  targetId: string,
  operatorConfirmed: boolean,
  signal?: AbortSignal
) {
  return http.request<FocusConfirmation>({
    method: 'post',
    url: '/api/hid/focus',
    data: { operationId, challenge, targetId, operatorConfirmed },
    headers: { 'Content-Type': 'application/json' },
    signal,
    timeout: 10000
  });
}

export function acquireLease(operationId: string, ttlSeconds: number, signal?: AbortSignal) {
  return http.request<LeaseGrant>({
    method: 'post',
    url: '/api/hid/lease',
    data: { action: 'acquire', operationId, ttlSeconds },
    headers: { 'Content-Type': 'application/json' },
    signal,
    timeout: 8000
  });
}

export function renewLease(
  operationId: string,
  leaseId: string,
  ttlSeconds: number,
  signal?: AbortSignal
) {
  return http.request<LeaseGrant>({
    method: 'post',
    url: '/api/hid/lease',
    data: { action: 'renew', operationId, leaseId, ttlSeconds },
    headers: { 'Content-Type': 'application/json' },
    signal,
    timeout: 8000
  });
}

export function reportHid(
  operationId: string,
  leaseId: string,
  report: HidReport,
  signal?: AbortSignal
) {
  return http.request<LeaseGrant>({
    method: 'post',
    url: '/api/hid/lease',
    data: { action: 'report', operationId, leaseId, report },
    headers: { 'Content-Type': 'application/json' },
    signal,
    timeout: 8000
  });
}

export function releaseLease(operationId: string, leaseId: string, signal?: AbortSignal) {
  return http.request<LeaseGrant>({
    method: 'post',
    url: '/api/hid/lease',
    data: { action: 'release', operationId, leaseId },
    headers: { 'Content-Type': 'application/json' },
    signal,
    timeout: 8000
  });
}

export function submitPowerAction(
  operationId: string,
  action: 'power_short' | 'power_long' | 'reset',
  signal?: AbortSignal
) {
  return http.request<PowerResult>({
    method: 'post',
    url: '/api/application-power',
    data: { operationId, action },
    headers: { 'Content-Type': 'application/json' },
    signal,
    timeout: 20000
  });
}
