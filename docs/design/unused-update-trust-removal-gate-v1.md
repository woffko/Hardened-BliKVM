# Unused application public-trust removal gate v1

## Purpose and applicability

This gate removes one exact application-update supervisor configuration only
when its private signing key is no longer usable and the enrolled public trust
root has never acquired update authority. It exists to return safely to the
initial no-trust state before a separately reviewed initial enrollment.

It is not a general key-rotation or revocation mechanism. It refuses operation
after application state, slots, update runtime, an active/enabled update unit,
or an unexpected configuration exists. A later versioned contract is required
for rotation after update state has been initialized or used.

## Accepted evidence

The host tool requires the private mode-0600 receipt from the completed initial
enrollment and a separately supplied exact configuration SHA-256. The receipt
must prove public-key-only enrollment, persistent configuration, absent
state/runtime, inert units, removed guards, and no private-key, reboot, or
target-power-control action. A mismatched, writable, linked, foreign, oversized,
failed, or incomplete receipt is rejected before device contact.

The old private key is never an input. It remains outside the repository,
`/tmp`, Project Memory, device staging, and the test device and is neither read
nor modified by this gate.

## Read-only preflight

The preflight validates the exact root-owned mode-0600 single-link active
configuration and rejects hidden replacement parts. It also requires:

- the exact accepted runtime and inert update-runtime packages;
- absent application update state, slots, and runtime roots;
- every update/current/candidate unit inactive and not enabled;
- healthy persistent services and exact PID/restart counters;
- unchanged application account, Nginx, TLS, listeners, and control boundary;
- a writable root filesystem and idle package manager; and
- no trust-enrollment, package, runtime-upgrade, reboot, or web-cutover guard.

`--preflight-only` returns before any mutation and records
`deviceChanged=false`.

## Exact removal and failure state

The mutating path repeats all preconditions and captured runtime facts in a new
SSH transaction immediately before one exact unlink of
`/etc/hardened-blikvm/update-supervisor.json`. The path, file type, owner, mode,
link count, and SHA-256 are revalidated first. The configuration parent is
synchronized, and both ordinary and dangling-link absence are required.

The unlink is the only persistent mutation. Both possible interruption states
are fail-safe for this narrowly unused boundary: the exact old public config is
still present, or no active public config exists while all update authority is
absent. There is no partial configuration content to restore, and restoring a
trust root whose private key is unusable is not a useful success condition.
On failure, a separate read-only transaction classifies the active path as the
exact old config, absent, or unknown; unknown state is a hard stop.

After removal, a new SSH transaction independently verifies configuration
absence, unchanged runtime/account/Nginx/TLS/PIDs/restarts, absent update
state/slots/runtime, inert units, and clean guards. The receipt records no
runtime restart, unit enablement, state initialization, reboot, or target power
control.

## Next boundary

A new key requires its own backup, public fingerprint and exact-request review,
hash-named local request snapshot, read-only enrollment preflight, and ordinary
guarded initial enrollment. This removal gate cannot select, upload, validate,
or publish the replacement key and therefore cannot silently rotate trust.
