const MIN_RTC_UNIX_SECONDS = 946_684_800;
const MAX_RTC_UNIX_SECONDS = 4_102_444_799;
const MAX_RTC_OFFSET_SECONDS = 315_576_000;

export function formatRtcUnixSeconds(value: number | null | undefined) {
  if (value === null || value === undefined || !Number.isInteger(value)) return '-';
  if (value < MIN_RTC_UNIX_SECONDS || value > MAX_RTC_UNIX_SECONDS) return '-';
  return new Date(value * 1000).toISOString().replace('T', ' ').replace('.000Z', ' UTC');
}

export function formatRtcOffsetSeconds(value: number | null | undefined) {
  if (value === null || value === undefined || !Number.isInteger(value)) return '-';
  if (Math.abs(value) > MAX_RTC_OFFSET_SECONDS) return '-';
  if (value === 0) return '0s';

  const sign = value < 0 ? '-' : '+';
  let remaining = Math.abs(value);
  const days = Math.floor(remaining / 86_400);
  remaining %= 86_400;
  const hours = Math.floor(remaining / 3_600);
  remaining %= 3_600;
  const minutes = Math.floor(remaining / 60);
  const seconds = remaining % 60;
  const parts = [
    days ? `${days}d` : '',
    hours ? `${hours}h` : '',
    minutes ? `${minutes}m` : '',
    seconds ? `${seconds}s` : ''
  ].filter(Boolean);
  return `${sign}${parts.join(' ')}`;
}
