# Restore timer arm/disarm acceptance

On 2026-09-08, device job `920c71c3aa024bbba948a8e6077963dd` passed the
status-restore timer arm/disarm probe. It never stopped the status socket.
The complete host check had passed in job `5f58f947269d48efbed98db8f79286aa`.

Before timer creation, the probe verified the retained sequence-five status,
serving process identities and an idle `active/listening` status socket. It
checked the system manager's unit search paths for existing units, aliases,
dependency directories and exact, dash-prefix or type-wide drop-in paths.
Only an unused namespace without those inherited configurations was accepted.

The new timer and service were verified through selected typed D-Bus
properties: exact identities, transient ownership, no drop-ins, expected
activation states, one monotonic 120-second expression, no calendar/random
delay, and one unrun command starting the fixed status socket. Additional
execution hooks and alternate users, commands or restart policies were
rejected. The observed initial guard margin was 119 seconds.

An explicit idempotent start of the already-active status socket was followed
by unchanged status and stable boot/main-PID/start-time/restart-counter checks.
The timer was stopped, both temporary units became unloaded, and final status
and serving identity checks passed. No server or video restart was requested;
no restorer service was stopped. The receipt reports `guardMayRemain=false`.

The private receipt is `evidence/private/status-restore-timer-arm-disarm.json`.
Its schema and all seven module hashes were revalidated against the exact
local source after completion. Code was supplied in memory; no probe source
or credential was staged into a device file.

This proves timer creation, definition checking, explicit disarm/collection
and an idempotent start while status was already available. It does **not**
prove timer firing, recovery after controller loss, restoration from a real
status outage, authenticated browser stale/error/recovery behavior, or the
remaining broader soak and update-interface acceptance gates.
