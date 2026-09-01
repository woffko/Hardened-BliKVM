# Diagnostic encrypted store v1 design

Status: design boundary for an unlinked filesystem adapter. This document does
not authorize key-file loading, Platform Broker linkage, collector reads or
acknowledgements, HTTP, UI, packaging, enablement, or a production key.

## Fixed identity and paths

The only production root is `/var/lib/hardened-blikvm-diagnostics`. The process
must be the exact non-root `hardened-blikvm-broker` account with effective group
`hardened-blikvm`; both names are resolved through the system account database,
and zero or mismatched IDs fail before opening the store.

The pre-provisioned root is mode 0700 and owned by that UID/GID. The only
allowed entries are mode-0600 `journal-a.bin`, `journal-b.bin`, and transient
`.journal-a.tmp`/`.journal-b.tmp`. Copies and safe crash-left temporaries are
exactly bounded by the accepted 16,552-byte encrypted format. The store does
not create the root or accept a path, environment variable, configuration, or
user-controlled name.

## Directory and inode safety

The canonical root path and a no-follow directory descriptor must have the
same device/inode/owner/group identity. The descriptor is held under an
exclusive nonblocking `flock` for the store lifetime. The root is revalidated
before and after enumeration, reads, and replacements.

Copies and temporaries are opened relative to that descriptor with fixed
NUL-terminated names, `O_NOFOLLOW`, `O_CLOEXEC`, and `O_NONBLOCK`. They must be
regular files on the root device, have exactly one hard link, exact owner/group
and mode, no POSIX access ACL, and acceptable exact/bounded length. The root
also rejects access/default ACLs and every unexpected directory entry. Safe
partial temporaries can be removed and the directory synced only by an
explicit recovery-capable operation; unsafe ones stop recovery.

Opening the store is nonmutating. The explicit read-only load rejects even a
validated crash-left temporary as `RecoveryRequired` and never cleans or
repairs it. Initialization, update, repair, and the recovery-capable load may
remove only a validated temporary before their operation. This separates
inspection from recovery authority.

## Nonce and key boundary

Every API receives a typed caller-owned `DiagnosticJournalKeyRing`; the store
does not retain it or read any credential. Production nonce generation uses
`rand_core::OsRng` for one fresh 24-byte value per initialization or update.
Failure or an all-zero value fails closed. Tests alone can inject deterministic
nonces. The journal plan enforces active-key use and rejects reuse against
retained copies.

Key credential format, key ID provisioning, rotation ceremony, and systemd
delivery are a later independent boundary. Keys, nonces, ciphertext, plaintext,
bundle bytes, numeric identities, paths, and raw I/O errors are absent from the
store's `Debug`, `Display`, and error payloads.

## Atomic write protocol

Initialization and updates create one encrypted journal plan and therefore one
nonce/ciphertext. The exact bytes are committed first to the non-selected copy
and then to the selected copy; initialization uses A then B. Repair copies the
already authenticated selected bytes and consumes no nonce.

Each replacement performs, in order:

1. re-read target and remove only a validated leftover temporary;
2. `openat(O_CREAT|O_EXCL|O_NOFOLLOW)` one new mode-0600 temporary;
3. full write and flush;
4. file `sync_all`;
5. metadata, ACL, exact-length, and authenticated readback on the open inode;
6. same-directory atomic `renameat` to the fixed copy name;
7. directory `sync_all`;
8. no-follow reopen, metadata/ACL/readback, and authenticated equality check;
9. after both writes, full A/B reselection at the expected generation and
   exact typed journal comparison.

The selected ciphertext bytes as well as generation/index/degraded/plaintext
state are rechecked immediately before writing, so same-generation external
re-encryption is a concurrent change rather than an accepted replacement.

## Failure model

Tests inject failure after unsynced write, temporary sync, rename before
directory sync, and directory sync before final verification on both the first
and second copy. Before first rename, the prior clean generation remains.
After first rename, the new generation is selectable as degraded. After the
second rename, both copies may already be clean even when the caller receives a
failure. The store never translates any failure into collector action.

Corrupt or truncated safe copies are degraded content and can be repaired from
the authenticated peer. Wrong/missing keys, nonce failure/reuse, generation
race, rollback floor, root replacement, unsafe metadata/ACL/link/type, and
readback mismatch fail closed.

## Deferred boundaries

The first implementation is an unlinked library. A sibling unlinked loader now
parses one fixed systemd credential into the typed caller-owned key ring; see
`diagnostic-key-credential-v1.md`. Deferred work includes a production key
ceremony and persistent-source protection, external rollback floor,
native filesystem gate and maintenance acceptance, Platform Broker feature
integration, background reconciliation, write-readback-sync-ack coordination,
HTTP, UI, retention controls, packaging, privacy review, soak, and reboot.
