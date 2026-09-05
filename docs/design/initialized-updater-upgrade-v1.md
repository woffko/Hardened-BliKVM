# State-preserving updater dev2 to dev3 upgrade

This gate changes only the inert application-updater Debian package, while the
GUI continues executing the already confirmed signed application slot. It
must not reuse the earlier no-state, package-server dev1-to-dev2 preconditions.

The accepted dev3 package is built twice from `e371528fbb8ee2255dfc0822011aac6932457ad2`.
Both source bundles have SHA-256
`76032fd77ae32ccb9732db5d22addfb28bf238fc56408be24e9f68d09a90a90f`;
both packages have SHA-256
`688718d3adecddc61c6e183c0cd2a5888dc1dfa443ce8fc7b0170f4e249d9779`.
The supervisor hash matches native event acceptance:
`2e0fe93a2acf14a34d60f2495b64fc245ef77f9eac0ebd0716ca21ae0b02cdc1`.
Artifacts reside in the approved external build root under
`updater-dev3-e371528-{a,b}/`. This is not a new signed application bundle.

Required sequence:

1. Host validates exact old/new package hashes, complete file maps, metadata,
   versions, runtime dependency and absence of maintainer scripts/configuration.
2. Native preflight verifies the installed dev2 tree, signed-slot serving,
   generation/floor one, no pending activation, inert updater units, unchanged
   trust/TLS/accounts and clean dpkg state. Snapshot both persistent update
   roots, including file hashes, symlinks and ownership/permissions.
3. Copy both bound packages and a fixed rollback controller into a new
   root-owned private `/var/lib` guard. Synchronize recovery metadata and arm
   persistent deadline and early-boot recovery before installing anything.
4. Install only the bound dev3 package, reload systemd definitions, verify the
   complete installed file map and compare the preserved state/runtime snapshot.
   Do not enable the watchdog, confirmation socket or candidate infrastructure.
5. Rehearse explicit downgrade to the bound dev2 package; independently verify
   restoration and remove only the exact guard artifacts.
6. Repeat preparation, verify independently, atomically record retention,
   disarm recovery and remove the exact guard artifacts. A late rollback must
   honor the synchronized committed marker and must not downgrade retained dev3.

Recovery changes package-owned files only. It never removes application state,
slots, trust, accounts or TLS and never restarts the serving GUI or controls
target Power/Reset. During early-boot recovery, persistent-state and package
checks apply; comparisons to pre-reboot PIDs are inappropriate. Real candidate
activation and watchdog enablement remain subsequent gates.
