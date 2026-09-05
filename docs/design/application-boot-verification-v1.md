# Mandatory application verification before serving

Current/candidate service startup must not race the two-second watchdog after
boot. Updater dev5 introduces `hardened-blikvm-update-recover.service` as a
required, ordered dependency of both servers. It runs the existing Rust
`watchdog-once` recovery under updater identity with a read-only systemd
configuration credential and the same narrow state/slot/runtime write paths.

The oneshot remains active after success for that boot, avoiding an extra
recovery transaction on every live server switch. It has no `Condition` that
could turn missing trust/state into a successfully skipped prerequisite, and
no `OnSuccess` edge back to the services waiting for it. Missing/unsafe trust,
unverifiable confirmed payload or corrupt state must fail the dependency and
prevent server execution. Recovery may repair a pending boot-changed operation
before the selected pointer is used. Any queued confirmed-apply event is still
acknowledged by the existing event mechanism; no alternate journal writer is
introduced.

The isolated full-flow gate includes this real template and dependency graph.
With its timer stopped, it removes only the fixture trust configuration, then
separately alters a fixture web byte while leaving the executable intact.
Both cases must fail the recovery dependency with zero serving PID. Restoring
the exact fixture inputs must allow startup, followed by normal candidate
confirmation and timeout rollback tests.

The older package maps remain version-specific so exact dev1-dev4 rollback
packages are not incorrectly rejected for lacking a dev5 unit. This is source
and isolated-test work until native acceptance, dev5 packaging/deployment and
real reboot verification complete. Do not claim that `RemainAfterExit` or a
unit file alone proves either reboot behavior or interruption recovery.
