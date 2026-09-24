function record(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function resolverServers(value: unknown): string[] {
  if (
    !record(value) ||
    !Array.isArray(value.servers) ||
    value.servers.length > 8 ||
    !value.servers.every(
      (server): server is string => typeof server === 'string' && server.length <= 45
    )
  )
    throw new Error('Invalid resolver response');
  return value.servers;
}

export function dnsWriteVerified(value: unknown, requested: string[]): boolean {
  if (!record(value) || value.applied !== true || value.rejection != null) return false;
  try {
    const actual = resolverServers(value.readBack);
    return (
      actual.length === requested.length &&
      actual.every((server, index) => server === requested[index])
    );
  } catch {
    return false;
  }
}

/** A successful transport response alone does not prove the requested state. */
export function ipv6WriteVerified(
  value: unknown,
  interfaceName: string,
  disabled: boolean
): boolean {
  if (!record(value) || value.applied !== true || value.rejection != null) return false;
  const state = value.ipv6State;
  if (!record(state) || !record(state.interfaces)) return false;
  return (
    Object.prototype.hasOwnProperty.call(state.interfaces, interfaceName) &&
    state.interfaces[interfaceName] === (disabled ? 'disabled' : 'enabled')
  );
}
