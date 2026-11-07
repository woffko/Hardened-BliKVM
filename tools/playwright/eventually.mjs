// Waits for the contracts and probes, timed by performance.now(). This
// host's wall clock steps by about 3.6 s every 32 s, so a deadline taken from
// Date.now() can end a wait before a timer it waits for
// (tools/test_eventually.mjs).

/** Polls `condition` every `intervalMs` until it holds or `timeoutMs` passed. */
export async function eventually(condition, timeoutMs = 6000, intervalMs = 50) {
  const passed = deadline(timeoutMs);
  while (!passed()) {
    if (await condition()) return true;
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }
  return condition();
}

/** A function that says whether `ms` have passed since it was made. */
export function deadline(ms) {
  const end = performance.now() + ms;
  return () => performance.now() >= end;
}
