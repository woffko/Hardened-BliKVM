# Application initial state and slot gate v1

## Purpose

This gate creates the first production-signed immutable application slot and
initial update journal without changing the application process that currently
serves the device. Public-trust enrollment must already be accepted. Update
units remain inactive and not enabled throughout this gate.

The gate does not activate a candidate, restart the server, switch Nginx,
enable the watchdog/socket/timer, expose an update route, reboot the device, or
invoke ATX Power/Reset or another attached-target power control.

## Initial payload provenance

The sequence-one slot must be byte-equivalent to the accepted running
application payload. It is built from the exact reproducible
`hardened-blikvm-runtime_0.1.0~dev2_arm64.deb`, not copied back from a mutable
live filesystem and not reconstructed from the current source checkout.

`tools/build_production_update_bundle.py` first applies the existing strict
Debian package verifier with the exact package, source-bundle, product-source,
builder, version, and architecture bindings. It then reverse-maps only:

- the server binary to `bin/bli-kvm-server`;
- the platform, video, and fail-closed control brokers to `libexec/`; and
- the complete installed web tree to `share/web/`.

Every selected member is compared with the already validated package file
mode, size, and SHA-256. Duplicate paths, non-regular selected members,
incomplete extraction, mapping collisions, missing required files, or payload
bounds fail closed. Systemd units, package metadata, documentation, active
configuration, credentials, mutable state, and hardware backends do not enter
the application bundle.

## Production signing boundary

The builder consumes the exact reviewed public enrollment request and derives
the encrypted private-key location from the local operator identity and key
ID. The private-key path, bytes, and passphrase are not accepted through CLI
arguments, environment variables, stdin, source, receipts, or logs. OpenSSL
opens the encrypted mode-0400 key through an inherited descriptor and reads the
passphrase directly from the controlling terminal.

The canonical manifest binds format/product/target/architecture, key ID,
version, sequence one, minimum system sequence one, the accepted runtime source
commit, and every payload path/mode/size/hash. The raw Ed25519 signature is
immediately verified against the enrollment public key before a deterministic
uncompressed USTAR bundle is written. The output receipt contains public
hashes and explicitly records that an owner-only Linux snapshot is still
required when the build artifact resides on the Windows-mounted staging disk.

Before device contact, the exact bundle must also pass the same Rust verifier
and stager in disposable owner-only host roots. Two separately constructed
production bundles should be byte-identical because canonical Ed25519 signing
is deterministic; mismatch is a hard stop.

## Read-only device preflight

The device preflight must bind the accepted trust-config hash, installed
runtime/update-runtime package versions, complete current runtime file hashes,
and the exact sequence-one bundle. It confirms:

- `/var/lib/hardened-blikvm-slots`, `/var/lib/hardened-blikvm-update`, and the
  update runtime roots are absent;
- every update/current/candidate unit is inactive and not enabled;
- package management is idle and the root filesystem is writable;
- the dedicated update identity and shared runtime group are exact;
- the application account, Nginx, TLS, listener, PID/restart, KVMD, health,
  and absent-control-boundary facts are unchanged; and
- no trust, package, reboot, activation, or initialization guard remains.

The preflight returns before upload or mutation and records
`deviceChanged=false`.

## Guarded stage and initialization

The mutating gate creates one marker-owned staging root, uploads only the
hash-bound public signed bundle, and validates that its payload hashes equal
the installed runtime package before creating persistent roots. It then arms a
synchronized early-boot rollback and a ten-minute timer.

The slots root is created mode 0750 and the state root mode 0700, both owned by
the dedicated update identity with the accepted shared group. The supervisor
runs as that identity under a bounded transient systemd sandbox. `stage`
publishes the immutable signed slot without selecting it. `initialize` accepts
only that exact slot, writes generation-one state durably, and atomically sets
`current` to the same slot. A read-only `status` must report identical active
and confirmed slot references, no previous slot, no pending operation, and
sequence/floor values of one.

The currently enabled package-backed `hardened-blikvm-server.service` remains
the serving process. Current/candidate server templates and every update unit
stay inactive and not enabled, so initialization grants no network update
surface and performs no application transition.

## Rollback and acceptance

Before retention, rollback may remove only roots and staging owned by the
exact run marker. It validates path, filesystem, owner, type, link count,
bounded tree shape, signed slot proof, journal hash, and current pointer before
making read-only slot contents owner-writable for deletion. It refuses foreign,
unexpected, or concurrently changed material and retains evidence instead of
guessing.

Success removes all guards and transient assets, opens a new SSH transaction,
revalidates the signed slot, journal, pointer, package equivalence, inert units,
and unchanged serving runtime, and retains only the initialized roots. Later
candidate activation, health confirmation, rollback under process failure,
interruption/reboot matrices, update API/GUI, rotation/revocation, and release
acceptance remain separate gates.
