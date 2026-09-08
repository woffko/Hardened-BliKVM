# Status outage: read-only systemd preflight

On 2026-09-08, job `b884270874cc4e70b037ed5b639245a7` read ten fixed public
D-Bus properties on the test device. It created no timer, stopped no socket
and changed no service or configuration. Systemd reported version
`249.11-0ubuntu3.17`. The status socket was `active/listening`, with `Accept`
true, a four-connection limit and zero current connections at observation.

`busctl --system --json=short get-property` returned an object containing
exactly `type` and `data`. Scalars are direct values, not singleton arrays:

| Signature | Observed data shape |
| --- | --- |
| `s` | String, such as `active` |
| `b` | JSON boolean |
| `u`, `t` | JSON integer |
| `a(stt)` | Array of `[base, durationUsec, nextElapseUsec]` entries |

The existing `systemd-tmpfiles-clean.timer` was used only as a read-only wire
format reference. Its monotonic deadline was nonzero, its realtime deadline
was zero, and `WakeSystem` was false. Its two timer expressions are not a valid
substitute for the planned single-expression outage guard.

The strict decoder rejects boolean-as-integer values, overflow, unexpected
signatures and malformed timer entries. The guard deadline helper accepts
only one `OnActiveUSec` expression of the policy's configured duration, matching
`NextElapseUSecMonotonic`, with `WakeSystem=false`. Remaining time is calculated
against the device's `CLOCK_MONOTONIC` and rounded down. Realtime must not be
used for this `OnActiveSec` guard; these timers are independent of wall-clock
adjustments. See the [upstream systemd timer documentation](https://github.com/systemd/systemd/blob/main/man/systemd.timer.xml).

The host revalidated the private receipt against the strict type decoder and
verified the exact remote source hash:
`f62285d59ae2010ade2f517592a74b87ca2d6c844f72ff02dc0e969a1d048995`.
Receipt: `evidence/private/status-systemd-wire-preflight.json`.
The complete host gate passed in job `761eaf6ebeda4d699e3669c574946393`.

Transient guard creation, ownership/definition checking and explicit disarm
subsequently passed in [the arm/disarm probe](status-restore-timer-arm-disarm.md).
Post-stop worker draining, elapsed-guard handling on the device, authenticated
browser outage/recovery and automatic restoration after coordinator loss remain
unverified. A successful property read or pure policy test does not prove those
device gates.
