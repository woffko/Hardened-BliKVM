# Retained updater dev4

The H616 retained updater `0.1.0~dev4` on 2026-09-05. This adds the accepted
candidate reporter credential alias to the previously retained event-driven
confirmed-apply implementation. Source/package bindings and reproducibility
evidence are in `../design/initialized-updater-upgrade-v1.md`.

Rehearsal job `f284cc5c011c41738af5caefd665d4e9` installed dev4, restored
dev3, independently verified restoration and removed its guards. Retaining
job `da85227da39e4ecfb7b98bc14722e036` repeated the upgrade, committed it,
removed guards/staging and independently verified the final state. Receipts:
`evidence/private/device-updater-dev4-{rehearsal,retained}.json`.

The full installed file tree matched the bound dev4 package. Persistent update
state/slot inode, ownership, permission, hash and symlink snapshots matched;
production generation/floor remain one. GUI PID 405007, Nginx, broker and video,
TLS, account and public trust were preserved. Neither test controlled target
Power/Reset.

The real watchdog and confirmation socket are still disabled. Next is guarded
idle infrastructure enablement, including the packaged tmpfiles example as an
active local configuration, followed by real production-key application update
and browser/recovery acceptance. Candidate/rollback tests so far used isolated
roots and an ephemeral test signing key, not the production update journal.
