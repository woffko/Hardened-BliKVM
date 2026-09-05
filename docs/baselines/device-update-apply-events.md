# Native confirmed-apply event acceptance

On 2026-09-05 the isolated H616 systemd gate passed twice with supervisor
source `45009e5`, SHA-256
`2e0fe93a2acf14a34d60f2495b64fc245ef77f9eac0ebd0716ca21ae0b02cdc1`.
The final run used the packaged apply/watchdog command sequence without the
temporary metadata-only diagnostic command.

- Five clean timer invocations preserved the initial test-process PID.
- Confirmation applied the second signed fixture slot and acknowledged its
  generation-bound request; five further clean invocations preserved its PID.
- An injected test-service start failure preserved a rollback apply request.
- Removing that injected failure allowed a subsequent timer retry to apply
  and acknowledge the request; five further invocations preserved the PID.
- Independent before/after snapshots matched for the actual serving GUI,
  initialized production application state, public trust, TLS and accounts.
- Fixture services, fixture data and controller staging were removed. No
  attached-target power control was exercised.

Final job: `b6f21b9d31c844c48cc651ceaed367d4`; receipt:
`evidence/private/device-apply-events-exact-units.json`. The preceding
instrumented pass was `d9873487d05b438eb564805f70609e4c`.
Full host validation passed in `8a734ab9fede4eecb87761aa17ddce67`;
ARM64 build passed in `4f04ecc4071a4be3a8fcd59d1661353e`.

The target systemd 249 masks credentials from control commands. The accepted
unit snapshots both credentials before its fixed privileged restart and
bind-mounts only its own credential directory read-only at the separate fixed
apply-credential root for the unprivileged ACK. Original ownership, ancestor,
single-link, no-follow and read-only checks remain enforced. See
`../design/update-service-application-v1.md` for the trust boundary and sources.

This proves isolated event delivery and retry, not installed dev3 acceptance.
The device still runs updater dev2 with the real watchdog and confirmation
socket disabled. Next: reproducible dev3 packaging, rollback-gated package
upgrade preserving initialized state and signed-slot serving, then real
candidate health/rollback and updater activation acceptance.
