# Retained idle updater infrastructure

On 2026-09-05 the actual dev4 watchdog timer and confirmation socket were
enabled and retained after a complete enable/disable rehearsal. The active
local tmpfiles configuration is the exact packaged example, SHA-256
`93081586b0cd99105f743bed2d4d81c78ef687aa338a6541db417f2dc2a30588`.
It provisions only the existing slot root and the correctly owned/grouped
confirmation socket directory; no account, TLS or trust content is included.

Rehearsal `c992879dfaf845b791b9ceda26564b50` verified six distinct real
watchdog invocations, unchanged serving runtime/state, successful disabling
and removal of the rule/empty runtime directories. Retention
`bfe86aa4cd3542a0a88defb39506f959` repeated six invocations, verified again
in separate SSH transactions, committed enablement and removed all guards
and controller staging. Receipts:
`evidence/private/device-updater-infrastructure-{rehearsal,retained}.json`.

The GUI still uses signed slot one, PID 405007. Nginx, broker/video, accounts,
TLS/public trust and persistent state/slot fingerprints remained unchanged.
Generation/floor remain one, with no pending transaction, health token or
confirmed-apply request. The socket, its parent and the watchdog runtime
directory passed exact UID/GID/mode checks. No candidate activation or target
Power/Reset control occurred in this gate.

The previous inert-only bootstrap/package verifiers must not be used unchanged
while this timer owns its runtime directory. Status queries must not create
and then remove that shared directory. Next is the installed production-key
update flow, timeout recovery, successful confirmation and browser acceptance;
reboot/interruption and remaining project functionality are still outstanding.
