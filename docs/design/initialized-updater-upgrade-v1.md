# State-preserving updater package upgrade

## Current dev4-to-dev5 operational transition

The gate now binds retained dev4 to dev5, preserving the already enabled
watchdog/socket and confirmed application sequence two. The observer uses the
operational snapshot without deleting the watchdog runtime directory and
without assuming generation one. Original helper dependencies are copied
uniquely into the guard. Version-specific file maps also require the added
dev5 recovery unit to be absent after an explicit dev4 downgrade.

Dev5 source/builder: `17eb3c42c9cfc30cd474e66316bd97a5c396a684`.
Both source bundles: `c704dbd223cda0dd0025677d3abdf497f19323071a92ca7e7c3bcb34b08fe9de`.
Both packages: `da88afb8c4b97d1f71318f04afa44445b1f64d76c6bf6580ecfa4200e67099f4`.
Build jobs: `2668a48d7e5b45d38df4e50250d958db` and
`87a4d569f9e343669e7824b6e0aaec38`; paths use
`updater-dev5-17eb3c4-{a,b}/` in the approved external build root.
The package rollback boot guard is ordered before both application servers;
its boot branch verifies package/persistent state without requiring running
GUI processes. Native package upgrade/rehearsal and real reboot acceptance
must still prove these source changes.

## Previous dev3-to-dev4 transition

The first dev2-to-dev3 transition below passed and its guards were removed;
see `../baselines/device-updater-dev3.md`. The same controller was then
explicitly bound to dev3-to-dev4. Existing receipts for an earlier package pair cannot
authorize retaining the new pair; a new rollback rehearsal is required.

Current dev4 source/builder: `bf956b16f752fa53e950bd9ee01339cd665e52ed`.
Two independent source bundles match SHA-256
`17b7d77b3696da6ba83ebd8abb77377280a627a731c8d6b30856f55ce5157737`;
both dev4 packages match
`fc63f2ee439fc5504526ba644544335a8f7e7eaea2cce95254fa39cd5ad98a3a`.
The previous retained dev3 package remains the exact rollback artifact.
The supervisor is unchanged; candidate reporter/template compatibility is the
new deployed behavior. Paths use `updater-dev4-bf956b1-{a,b}/` beneath the
approved external build root. Build jobs are `b9ce492e2b674506990ef2606666bf00`
and `098a7c99298e4a1bb5967ef54307b63b`.

## Original dev2-to-dev3 procedure and artifact history

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
