# Synthetic diagnostic persistence H616 gate design

Status: unshipped test harness and rollback orchestration only. This design
does not authorize a production key, rollback floor, executable, service,
broker feature, package, HTTP route, UI, reboot, or hardware action.

## Exact test boundary

`bli-kvm-diagnostic-persistence-gate` is a zero-argument AArch64 test
executable. It uses the real fixed key loader, fixed encrypted store, fixed
export client, persistence adapter, coordinator, journal, and typed export
protocol. It has no configurable path, socket, credential, key, binding, floor,
timeout, or endpoint.

The static source unit is not installed or enabled. During the approved test it
is copied temporarily to the exact runtime name
`hardened-blikvm-diagnostic-store-init.service`; that is one of the two
credential-directory identities already accepted by the key loader. The unit
runs as the exact broker UID/shared GID, loads one ephemeral synthetic
credential, exposes only the two fixed diagnostic roots as writable, denies
network/devices/capabilities, and has no `[Install]` section.

The test floor exists only as explicit generation values in the gate process.
It is not a production or rollback-independent anchor and is never persisted.

## Scripted protocol sequence

The gate owns a typed in-process Unix server at the export client's fixed
socket. It encodes responses through the accepted export protocol and contains
two fixed sealed windows. No raw log or free-form payload is accepted.

Starting from a freshly initialized encrypted generation one, the exact flow
is:

1. read window one and persist pending generation two;
2. advance the synthetic floor, reread, acknowledge, and persist acknowledged
   generation three;
3. read window two and persist pending generation four;
4. advance the floor, reread, start acknowledgement, deliberately stall the
   response, signal that the ACK request reached the stalled server, and only
   then cancel the runtime poll;
5. recover without collector I/O and persist ambiguous generation five;
6. advance the floor, reread the same window, and persist pending generation
   six;
7. advance the floor, reread and acknowledge exactly once, producing
   generation seven; and
8. read explicit no-sealed state and confirm idle generation seven.

Expected transport totals are seven reads and three acknowledgement requests.
The stalled acknowledgement does not apply, and there is no automatic retry.
The final clean journal contains two acknowledged entries. The gate writes one
fixed 171-byte semantic result with no bundle, binding, key, nonce, path,
identity, or raw error.

## Rollback orchestration

The host tool accepts only a hash-named static-PIE binary snapshot and the exact
committed gate-unit source. SSH host verification remains strict, and the
test-only password reaches `sshpass` only through the inherited secret file
descriptor supplied by Project Memory/Longrun.

Preflight requires the production/runtime roots, credential, target binary,
runtime unit, identities, watchdog, and marker stage to be absent. Rootfs must
remain read-write and KVMD/KVMD Nginx active.

Before any live account or persistent-path write, the tool:

- copies all four account databases into a mode-0700 marker-owned stage;
- predicts the exact `systemd-sysusers` transition in a synthetic root;
- stores before/after content hashes and selected numeric IDs;
- writes a self-contained rollback script; and
- arms a ten-minute systemd watchdog.

Rollback stops the test unit, validates exact marker/hash/mode/owner/link/size
state, removes only the fixed runtime socket/result, encrypted A/B/temp files,
synthetic credential, runtime unit, and gate binary, then restores account
databases only when their current hashes equal the predicted test state. A
concurrent database change stops restoration instead of being overwritten.
The stage cleanup explicitly models `.pwd.lock` and all four sysusers backup
files, then restores prior parent-directory absence/presence and confirms the
watchdog is inactive.

No recursive broad deletion, physical Power/Reset, reboot, shutdown, KVMD
restart, vendor log, hardware control, clock write, or network mutation is
permitted.

## Acceptance and deferred work

Host acceptance requires Rust tests/Clippy, generated-shell syntax and exact
allowlist tests, BasedPyright, systemd verification, full project checks,
RustSec, and two independent AArch64 gate builds. Native acceptance additionally
requires the private allowlisted receipt to prove generation seven, two
acknowledged entries, exact request counts, complete rollback, active KVMD
services, rootfs read-write, and final state absence.

Even a successful gate proves only ordinary H616 systemd/Unix/filesystem and
synthetic cancellation behavior. Production key custody, a trusted external
floor, power-cut/reboot durability, a shipped runtime, authenticated diagnostic
API, localized dedicated Diagnostics tab, packaging, privacy/soak, and release
remain separate gates.
