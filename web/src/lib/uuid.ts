/**
 * A version 4 UUID from the browser's cryptographic generator: its own
 * `randomUUID` where it has one (secure contexts only), otherwise 16 bytes
 * of `getRandomValues`, which every context has.
 */
export function generateUUIDv4(
  source: Pick<Crypto, 'getRandomValues'> & Partial<Pick<Crypto, 'randomUUID'>> = crypto
) {
  if (typeof source.randomUUID === 'function') {
    return source.randomUUID();
  }
  const bytes = source.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
