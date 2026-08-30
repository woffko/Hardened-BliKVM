export const HARDENED_NAME = 'Hardened BliKVM';
export const HARDENED_SHORT_NAME = 'Hardened-BliKVM';
export const HARDENED_VERSION = '0.1.0-dev';
export const HARDENED_LOGO_SRC = '/hardened-blikvm-logo.png';

export function formatHardenedVersion(version?: string) {
  const value = version?.trim();
  if (!value) return HARDENED_VERSION;
  return value;
}
