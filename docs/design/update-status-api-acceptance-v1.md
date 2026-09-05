# Reversible installed status API and GUI acceptance

## Fixed starting state

Require updater dev6 and the accepted supervisor hash, confirmed application
sequence four and generation sixteen, no pending update, active confirmed
server/Nginx, unchanged account/TLS/trust/broker/video state, and no unfinished
application transaction. The installed status socket must be disabled/inactive
with no active status-worker instance or endpoint. Its installed worker/socket
bytes must match the dev6 package. No package, application slot or trust change
belongs to this gate.

## Temporary wiring

Create a unique root-owned mode-0700 guard under the fixed
`/var/lib/hardened-blikvm-status-api-` namespace. Save only hashes, fixed file
bindings and non-secret state observations in its private marker. Arm a bounded
deadline restore service before modifying runtime wiring.

Use exact temporary drop-ins below `/run/systemd/system` for the confirmed and
candidate server units. Add only the fixed
`BLI_KVM_UPDATE_STATUS_SOCKET=/run/hardened-blikvm-update-api/status.sock`
environment value and a Wants/After dependency on the status socket. Do not add
Requires, apply/recovery dispatch, writable paths, credentials or new capability
privileges. The candidate remains inactive throughout this test.

Start the installed status socket without enabling it. Restart only the
confirmed server after `systemctl daemon-reload` has loaded both drop-ins,
so it consumes the temporary environment. Nginx, KVMD, broker,
video, application state and selected/confirmed slots must remain unchanged.
Only the confirmed server PID and its own restart count may change. Verify the
actual service environment, socket ownership/mode and unchanged updater state.

## Browser acceptance

Use the existing encrypted test-only web account through secret stdin and the
installed Playwright Chromium. Bind every report to the guard and test phase.
Never record passwords, cookies, tokens, endpoint credentials or private paths.

Before login and after logout, GET `/api/application-update/status` must return
401. After login, require exact string counters, generation `16`, floor `4`,
equal selected/confirmed sequence-four metadata and no pending operation.
Compare this view to the independently observed native manager status. Repeated
reads must not change any updater state or dispatch apply services.

Open Settings/Update on desktop and a mobile viewport. Require visible current
and confirmed version/sequence, read-only notice, Refresh behavior and no
upload/activation controls. Retain existing login, MJPEG decode/reconnect,
Diagnostics, logout and stream-revocation checks. No legacy updater routes,
control writes or WebSocket traffic are allowed.

## Restoration and cleanup

The deadline handler removes only exact matching temporary drop-ins, reloads
systemd, restarts the confirmed server when needed and stops the status socket.
It must not rewrite the journal, change a slot pointer, disable existing updater
watchdog/confirmation infrastructure, restore an old application or stop Nginx.
Foreign or modified files require inspection rather than deletion.

Runtime drop-ins and the non-enabled socket do not persist across reboot;
interrupted cleanup may retain a private guard directory for later inspection.
Do not infer cleanup from disappearance of the deadline timer alone.

After explicit restoration, inspect native state and repeat browser acceptance
with status wiring absent. The update panel must no longer be advertised, while
normal KVM/Diagnostics remain functional. Only then remove the exact known
guard files and verify their absence in a separate observation. Keep source
artifacts and test receipts. Persistent status configuration is a subsequent
rollback-rehearsed step, not an implied result of this temporary test.
