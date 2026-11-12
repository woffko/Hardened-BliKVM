import { http } from '@/lib/http.ts';

/**
 * The target's case LEDs: the power LED, and the disk's, lit while the disk
 * works. Each is null when the board does not have it or nothing answered.
 */
export type CaseLeds = {
  powerLed: boolean | null;
  hddLed: boolean | null;
};

export function getCaseLeds() {
  return http.get<CaseLeds>('/api/atx/leds');
}
