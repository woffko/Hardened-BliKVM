# Retained updater dev3 with initialized state preserved

The H616 retained updater `0.1.0~dev3` on 2026-09-05 after a complete
dev2-to-dev3-to-dev2 rehearsal and a second verified retaining installation.
The exact package and source bindings are recorded in
`../design/initialized-updater-upgrade-v1.md`.

Evidence:

- Host full checks: `d5008659740d4c3793f5326d0f7d778d`.
- Device preflight: `d3b93b9c86444bf6919e68567200335f`.
- Upgrade and explicit restoration: `ab024b8e980740e4ba26e130a2165d79`.
- Retention and independent final verification: `2e9df1c6b96a410aa995b0e72df7224b`.
- Private receipts: `evidence/private/device-initialized-updater-{preflight,rehearsal,retained}.json`.

Both persistent update roots preserved all captured inodes, ownership, modes,
regular-file hashes and symlink targets. Journal generation/floor remain one;
the confirmed signed slot is unchanged and no activation is pending. Server
PID 405007, Nginx, broker and video processes, accounts, TLS and public trust
remained unchanged. The exact installed package tree passed verification.
All upgrade guards and controller staging were removed; host rollback packages
remain available. No target Power/Reset control was used.

The watchdog and confirmation socket remain inactive/disabled. Candidate
activation, timeout rollback and operational updater enablement are not yet
accepted. The candidate health reporter also executes in `ExecStartPost` and
needs the same systemd 249 credential-namespace compatibility treatment before
its real service path can be accepted. Do not enable that path from this
checkpoint alone.
