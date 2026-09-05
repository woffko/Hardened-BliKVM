# Retained updater dev5

Dev5 is retained on H616 with the mandatory signed-state recovery dependency
installed for current/candidate server startup. Exact source/package hashes
are recorded in `../design/initialized-updater-upgrade-v1.md`.

Full host job `2c8ce7afb16b4a2aaa5149dbabd12425` passed. Rehearsal
`a3d63ee478ef4ab58b69a6a65d8c720b` installed dev5 and restored dev4,
including absence of the extra dev5 recovery unit after downgrade. Retention
`89cb86c5da354d1686fcaa964e2faee2` installed dev5 again, committed and
independently verified it, then removed all guard/controller staging.
Receipts: `evidence/private/device-updater-dev5-{rehearsal,retained}.json`.

The operational snapshot was preserved throughout: sequence-two signed GUI,
generation seven/floor two, running watchdog and confirmation socket,
application state/slot fingerprints, serving PID, Nginx, peers, accounts,
TLS/public trust and local tmpfiles policy. No target power control was used.

This was package installation without restarting the GUI. The newly required
boot verification has passed isolated native failure/restore tests, but a real
clean reboot and interrupted-update reboot still require acceptance.
