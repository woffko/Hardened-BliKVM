# Exact-template isolated activation flow acceptance

On 2026-09-05 job `d1d35e1b8a034671981a13679519c84c` passed the complete
isolated activation flow on H616. The receipt is
`evidence/private/device-activation-flow-isolated.json`.

The gate rendered all nine current package templates into unique temporary
services, preserving actual dependencies between activation, candidate apply,
candidate server, confirmation socket/service, watchdog and confirmed apply.
Only fixture paths, unit names, loopback port and server-state root differed;
the running production services had no conflict/deactivation edge to fixtures.

Accepted behavior:

- Slot one ran the actual Rust server; slot two carried the same reviewed
  executable in a separately signed test manifest.
- Explicit activation switched to the real candidate service. Its reporter
  validated health and sent the one-use credential through the actual
  socket-activated confirmation template.
- Confirmation raised only the isolated journal floor to two, consumed the
  token, triggered confirmed application and acknowledged the matching request.
- A non-executable third slot failed to start. The real timer rolled it back
  after the ten-second test health window without raising the floor.
- The restored slot-two server was healthy; clean timer invocations after both
  confirmation and rollback preserved its PID.
- All fixture services, socket/runtime, staged slots and controller files were
  removed. The actual serving runtime and production generation/floor one were
  unchanged. No Power/Reset control was used.

Binary bindings match `device-candidate-health.md` and
`device-update-apply-events.md`. The fixture signing key existed only in the
host builder process; only its public key and signed artifacts were staged.
This acceptance uses an isolated trust domain, not the production signing key.

Next is reproducible updater dev4 packaging and a state-preserving dev3-to-dev4
upgrade with rollback rehearsal. Real installed updater enablement, a
production-key application update, browser continuity/reconnect, interruption
and reboot acceptance remain subsequent gates.
