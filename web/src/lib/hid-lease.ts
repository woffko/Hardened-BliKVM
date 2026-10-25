import * as api from '@/api/restricted-control.ts';
import type { ApiResponse } from '@/lib/http.ts';

/**
 * What carries the lease's requests to the server: the HTTPS API, or the
 * QUIC session's input stream (`lib/quic/lease.ts`). The requests and the
 * answers are the same either way.
 */
export type LeaseTransport = {
  acquire(
    operationId: string,
    ttlSeconds: number,
    signal?: AbortSignal
  ): Promise<ApiResponse<api.LeaseGrant>>;
  renew(
    operationId: string,
    leaseId: string,
    ttlSeconds: number,
    signal?: AbortSignal
  ): Promise<ApiResponse<api.LeaseGrant>>;
  report(
    operationId: string,
    leaseId: string,
    report: api.HidReport,
    signal?: AbortSignal
  ): Promise<ApiResponse<api.LeaseGrant>>;
  release(
    operationId: string,
    leaseId: string,
    signal?: AbortSignal
  ): Promise<ApiResponse<api.LeaseGrant>>;
};

export const httpsLease: LeaseTransport = {
  acquire: api.acquireLease,
  renew: api.renewLease,
  report: api.reportHid,
  release: api.releaseLease
};

const OPERATION_ID_BYTES = 16;
/** The server holds manual input back while the AI assistant has control. */
export const ASSISTANT_ACTIVE = 'assistant_active';
const RENEW_FRACTION = 0.5;

/**
 * The board answered an acquire with a refusal: another session controls the
 * target, or the board allows no control now. Unlike a lease lost on the way
 * (a QUIC session that reconnected), asking again at once does not help.
 */
export class LeaseNotGranted extends Error {}

export function newOperationId(): string {
  const bytes = new Uint8Array(OPERATION_ID_BYTES);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
}

/**
 * Keeps one HID lease alive: it acquires a broker lease, renews it before the
 * server-advertised TTL elapses, and always releases the lease on stop.
 */
export class HidLeaseSession {
  private leaseId: string | null = null;
  private ttlSeconds = 0;
  private renewTimer: ReturnType<typeof setTimeout> | null = null;
  private generation = 0;
  private requests = new Set<AbortController>();

  /**
   * The broker admits a lease only while the session's target attestation is
   * live, and that attestation expires long before a lease acquired once would
   * end. The session therefore refreshes it before it acquires or renews.
   */
  constructor(
    private readonly attest: () => Promise<void>,
    private readonly transport: LeaseTransport = httpsLease
  ) {}

  async start(ttlSeconds: number): Promise<void> {
    const generation = ++this.generation;
    await this.attest();
    this.requireGeneration(generation);
    const response = await this.request((signal) =>
      this.transport.acquire(newOperationId(), ttlSeconds, signal)
    );
    if (generation !== this.generation) {
      if (response.data.applied && response.data.leaseId) {
        void this.transport.release(newOperationId(), response.data.leaseId).catch(() => undefined);
      }
      throw new DOMException('HID lease start was cancelled', 'AbortError');
    }
    if (!response.data.applied || !response.data.leaseId) {
      throw new LeaseNotGranted(response.data.rejection ?? 'HID lease was not granted');
    }
    this.leaseId = response.data.leaseId;
    this.scheduleRenew(response.data.ttlSeconds ?? ttlSeconds);
  }

  async report(report: api.HidReport): Promise<void> {
    const generation = this.generation;
    const leaseId = this.leaseId;
    if (!leaseId) {
      throw new Error('HID lease is not active');
    }
    const response = await this.request((signal) =>
      this.transport.report(newOperationId(), leaseId, report, signal)
    );
    this.requireGeneration(generation);
    if (!response.data.applied) {
      // The lease is still live; only this report was held back.
      if (response.data.rejection === ASSISTANT_ACTIVE) throw new Error(ASSISTANT_ACTIVE);
      this.invalidateLease();
      throw new Error(response.data.rejection ?? 'HID report was not applied');
    }
  }

  async stop(): Promise<void> {
    this.generation += 1;
    this.clearRenewTimer();
    for (const request of this.requests) request.abort();
    this.requests.clear();
    const leaseId = this.leaseId;
    this.leaseId = null;
    if (leaseId) {
      const response = await this.transport.release(newOperationId(), leaseId);
      if (!response.data.applied) {
        throw new Error(response.data.rejection ?? 'HID lease release was not applied');
      }
    }
  }

  private scheduleRenew(ttlSeconds: number): void {
    this.ttlSeconds = ttlSeconds;
    const delayMs = Math.max(1, Math.floor(ttlSeconds * 1000 * RENEW_FRACTION));
    this.renewTimer = setTimeout(() => {
      void this.renew();
    }, delayMs);
  }

  private async renew(): Promise<void> {
    const generation = this.generation;
    const leaseId = this.leaseId;
    if (!leaseId) {
      return;
    }
    try {
      await this.attest();
      this.requireGeneration(generation);
      const response = await this.request((signal) =>
        this.transport.renew(newOperationId(), leaseId, this.ttlSeconds, signal)
      );
      this.requireGeneration(generation);
      if (!response.data.applied) {
        throw new Error(response.data.rejection ?? 'HID lease renewal was not applied');
      }
      this.scheduleRenew(response.data.ttlSeconds ?? this.ttlSeconds);
    } catch {
      if (generation === this.generation) {
        this.invalidateLease();
        void this.transport.release(newOperationId(), leaseId).catch(() => undefined);
      }
    }
  }

  private async request<T>(run: (signal: AbortSignal) => Promise<T>): Promise<T> {
    const controller = new AbortController();
    this.requests.add(controller);
    try {
      return await run(controller.signal);
    } finally {
      this.requests.delete(controller);
    }
  }

  private requireGeneration(generation: number): void {
    if (generation !== this.generation) {
      throw new DOMException('HID lease operation was cancelled', 'AbortError');
    }
  }

  private invalidateLease(): void {
    this.leaseId = null;
    this.clearRenewTimer();
  }

  private clearRenewTimer(): void {
    if (this.renewTimer !== null) {
      clearTimeout(this.renewTimer);
      this.renewTimer = null;
    }
  }
}
