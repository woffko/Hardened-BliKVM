# Unused updater dev1 to dev2 transition

The credential-aware supervisor and three service templates are packaged as
updater `0.1.0~dev2`. Two independent builds from `e1e8062` produced identical
packages with SHA-256
`b3c385adfbcdbe9d1932982b71a9551f7325f9b8467fc632e194009fc822c4e8`.
The prior dev1 rollback package remains bound to SHA-256
`a42ebcc0ee9e0f4c2e0ad3ca313e783261a41fad3317358da7a0bd0effa9da20`.

`tools/stage_device_update_runtime_upgrade.py` validates both full packages and
their source/builder manifests before device contact. The narrowly pinned
transition applies only to the unused updater: enrolled public trust must
match its accepted receipt, state/slots/runtime must be absent (including
dangling symlinks), and updater units/instances must be inactive and disabled.
The serving runtime, account, Nginx, TLS, PIDs and restart counters are captured.

`--preflight-only` returns without uploading or changing anything. A mutating
run stages both packages under one root-owned marker directory, installs an
early-boot rollback service and ten-minute timer, and only then invokes the
bounded package installation. A directory lock serializes installation,
retention and rollback. Rollback reinstalls the exact old package; it never
purges the updater or removes enrolled trust. Before retention, all installed
file hashes/modes and unchanged-runtime invariants must pass. A durable commit
marker prevents a delayed rollback from downgrading a retained package.

`--exercise-rollback` installs dev2, executes the restoration path and requires
a new SSH check to prove exact dev1 restoration, preserved trust, unchanged
serving runtime and removed guards. A later ordinary run retains dev2 and
performs the same independent final verification against its new package.

This transition does not start updater/current/candidate services, initialize
state/slots, alter the trust key, restart the serving application, reboot or
invoke target Power/Reset. Those boundaries remain separate. Unexpected state
or failed cleanup is recorded and must be investigated before another run.
