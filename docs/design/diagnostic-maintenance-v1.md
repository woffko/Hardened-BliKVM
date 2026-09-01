# Diagnostic maintenance v1 design

Status: design boundary for an unshipped empty-only maintenance executable and
two static one-shot units. This document does not authorize a production key,
credential source creation, service enablement, live collector coordination,
acknowledgement, repair/update command, HTTP, UI, package, or release.

## Exact commands

The binary accepts exactly one argument after its executable name:

- `initialize-empty`;
- `inspect-empty`.

There is no path, key, key ID, floor, generation, nonce, repair, update,
rotation, acknowledgement, collector, socket, endpoint, network, shell, or
process-control argument. Unknown, missing, or additional arguments fail before
opening the credential or store.

`initialize-empty` loads the fixed systemd credential, opens the fixed store,
requires complete copy absence, writes the canonical encrypted empty
generation one to both copies through the accepted atomic store protocol, and
then verifies it through the read-only path. Repeat initialization fails.

`inspect-empty` loads the same fixed credential and performs only
`load_read_only(minimum_generation=1)`. It accepts exactly a clean,
non-degraded, zero-entry generation-one journal. A temporary, degraded copy,
other generation, nonempty journal, wrong key, or unsafe filesystem state is an
error. It never removes a temporary, repairs a copy, writes, syncs, renames, or
advances state.

The only success receipt is fixed semantic text:

`command=<exact-command> result=empty generation=1 copies=healthy`

Errors and receipts contain no key/path/UID/GID/nonce/ciphertext/journal/I/O
details. The exact backend implements neither `Clone` nor `Debug` because it
owns loaded keys.

## Provisioning and unit boundary

Tmpfiles source creates only `/var/lib/hardened-blikvm-diagnostics`, mode 0700,
owned by `hardened-blikvm-broker:hardened-blikvm`. It creates no credential or
copy.

Two static, non-enabled one-shot units are the only intended entrypoints:

- `hardened-blikvm-diagnostic-store-init.service`;
- `hardened-blikvm-diagnostic-store-inspect.service`.

Both are root-triggered by the service manager but execute as the exact
unprivileged broker UID/shared GID. Both use one fixed
`LoadCredential=diagnostic-journal-keyring:/etc/hardened-blikvm/diagnostic-journal-keyring`
contract. The loader accepts only the corresponding two runtime credential
directories; Platform Broker has no key-loader authority yet.

Both units have private devices, mounts, network, and temporary storage;
strict system/home/kernel/proc protection; empty capability sets; closed device
policy; IP denial; namespace/SUID/personality/W^X restrictions; and a bounded
ten-second start. The init unit has one exact `ReadWritePaths` entry for the
diagnostics root. The inspect unit has no writable path exception. Neither unit
has an `[Install]` section.

The `/etc` source path is a provisioning contract only. No file, key generator,
serializer, default, or secret is included in source or bundle. On the H616,
systemd 249 protects its runtime copy but no accepted encrypted-at-rest or
hardware-backed persistent source ceremony exists.

## Native gate boundary

The first device gate may generate only an ephemeral synthetic fixed-format key
outside the repository, stage it without exposing bytes in argv/environment,
logs, receipts, or Project Memory, and bind it to a watchdog plus exact
marker/hash/metadata rollback. The gate may run both commands and inspect only
allowlisted semantic facts and ciphertext metadata.

Success must restore the original absence of the production root, persistent
credential source, maintenance binary, units, staging, and watchdog. Rootfs
stays read-write, KVMD and KVMD Nginx stay active, and no physical Power or
Reset action is permitted. This proves systemd credential metadata and native
filesystem/syscall behavior only, not a production key, external rollback
floor, power-cut durability, live acknowledgement, or packaging.
