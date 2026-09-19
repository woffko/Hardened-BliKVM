import * as api from '@/api/restricted-control.ts';

const OPERATION_ID_BYTES = 16;
const RENEW_FRACTION = 0.5;

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

  /**
   * The broker admits a lease only while the session's target attestation is
   * live, and that attestation expires long before a lease acquired once would
   * end. The session therefore refreshes it before it acquires or renews.
   */
  constructor(private readonly attest: () => Promise<void>) {}

  async start(ttlSeconds: number): Promise<void> {
    await this.attest();
    const response = await api.acquireLease(newOperationId(), ttlSeconds);
    if (!response.data.applied || !response.data.leaseId) {
      throw new Error('HID lease was not granted');
    }
    this.leaseId = response.data.leaseId;
    this.scheduleRenew(response.data.ttlSeconds ?? ttlSeconds);
  }

  async report(report: api.HidReport): Promise<void> {
    if (!this.leaseId) {
      throw new Error('HID lease is not active');
    }
    await api.reportHid(newOperationId(), this.leaseId, report);
  }

  async stop(): Promise<void> {
    if (this.renewTimer !== null) {
      clearTimeout(this.renewTimer);
      this.renewTimer = null;
    }
    const leaseId = this.leaseId;
    this.leaseId = null;
    if (leaseId) {
      await api.releaseLease(newOperationId(), leaseId);
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
    const leaseId = this.leaseId;
    if (!leaseId) {
      return;
    }
    try {
      await this.attest();
      const response = await api.renewLease(newOperationId(), leaseId, this.ttlSeconds);
      if (response.data.applied) {
        this.scheduleRenew(response.data.ttlSeconds ?? this.ttlSeconds);
      } else {
        this.leaseId = null;
      }
    } catch {
      // A failed renewal could be a lapsed confirmation, so the next report
      // re-confirms and re-acquires instead of reporting into a dead session.
      this.leaseId = null;
    }
  }
}
