export function getBaseUrl(type: 'http' | 'ws'): string {
  const protocol =
    type === 'ws'
      ? window.location.protocol === 'https:'
        ? 'wss:'
        : 'ws:'
      : window.location.protocol;
  return `${protocol}//${window.location.host}`;
}
