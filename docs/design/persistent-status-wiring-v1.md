# Persistent read-only status wiring

## Scope and prerequisites

The completed reversible cycle is recorded in
`../baselines/status-api-seq5-acceptance.md`. Require the same installed updater
dev6, confirmed sequence five, floor five, generation twenty-two, account/TLS/
trust/peer fingerprints and absence of old transaction guards. This change
does not install a package, change an application slot or floor, enroll trust,
enable an upload/activation route, or alter device-control permissions.

Persist only two exact root-owned mode-0644 drop-ins under
`/etc/systemd/system/hardened-blikvm-server-{current,candidate}.service.d/`.
They contain the already tested fixed status-socket environment and Wants/After
dependency. Both server roles need the same wiring for future application
transitions. Leave the socket disabled: the servers' Wants dependency starts
it as needed, including after boot. Do not add a Requires dependency; status
failure must not take down the KVM server. Prove that failure is reported as
unavailable in the UI while ordinary KVM remains functional.

## Guard and transaction

Use a new fixed `hardened-blikvm-status-retain-` namespace with a root-owned
mode-0700 persistent guard directory. Save a versioned, exact-slot-bound marker
and known helper files. Refuse pre-existing drop-in directories, runtime
overrides or unfinished guards. Arm durable deadline and early-boot restoration
before creating either persistent drop-in. File publication and marker
transitions must use no-follow/exclusive creation, explicit modes, file sync
and parent-directory sync; unknown partial files are never overwritten.

The boot guard runs after local filesystems and before the status socket and
both server roles, with default dependencies disabled and explicit shutdown
ordering. During boot recovery it must not synchronously start/restart a server
ordered after itself. It removes only matching uncommitted drop-ins, reloads
systemd, and leaves normal boot to start the original server configuration.
Post-boot observation verifies complete runtime recovery; the boot handler
does not claim browser or running-service acceptance.

The online deadline path restores exact matching files, reloads systemd,
restarts only the confirmed server, stops the status socket and verifies the
baseline. It does not stop Nginx/brokers, restore journals or touch slots.

Marker states are prepared, wired, restored and committed. Commit requires
independent native verification and a guard-bound enabled browser receipt.
Persist and sync committed state before disarming guards. If commit was saved
but disarm was interrupted, deadline/boot recovery preserves the persistent
drop-ins and removes only the obsolete guard. Repeated restore/disarm/cleanup
must be safe and must not overwrite foreign files. Cleanup removes only the
closed known guard file set after restored/committed verification.

## Acceptance order

1. Host transition/fault tests and systemd ordering validation.
2. Device preflight without persistent changes.
3. Guarded persistent wiring, enabled desktop/mobile browser acceptance,
   explicit restoration, restored browser acceptance and independent cleanup.
4. Repeat the guarded change, verify browser/native state and commit retention.
5. A separate guarded software reboot verifies persistent configuration,
   unchanged selected/confirmed state, correct service ordering and browser
   behavior. No attached-target Power/Reset action belongs to these tests.

Before retention, validate the early-boot recovery path on the test device or
an equivalent systemd boot fixture; syntax checks alone do not prove absence
of ordering cycles. No unattended configuration is accepted while a required
recovery/boot/browser gate remains unverified. Retain all failure receipts and
source artifacts; this is not a release-readiness claim.

## Host-only implementation checkpoint

`tools/persistent_status_contract.py` now renders the fixed boot/deadline units
and defines recovery/commit decisions without performing mutations. Its tests
cover uncommitted and committed phases, boot versus online actions, acceptance
requirements and namespace boundaries. `tools/verify_persistent_status_ordering.py`
uses `systemd-analyze verify` on an explicit minimal target graph and requires
rejection of a deliberately injected basic/sysinit ordering cycle. This does
not boot systemd, run a controller, or prove the actual device's boot behavior.
The external advisory review request was denied; no repository content was
sent in that request, and authorization is still pending for that route.
