# Atomic encrypted diagnostic store checkpoint

On 2026-09-01, source commit
`2efe1f739f763957a1741266e3e9ebfc616ee605` added the first concrete
filesystem adapter for the encrypted diagnostic journal. The
`bli-kvm-diagnostic-store` crate remains unlinked from every process, feature,
package, systemd unit, API, UI, and prototype artifact. Its production API
accepts a typed caller-owned key ring and semantic journal mutations only; it
does not load, retain, derive, log, or provision a key.

The accepted storage protocol and deferred boundaries are specified in
`../design/diagnostic-store-v1.md`. This checkpoint does not create the
production root, authorize a production key, connect collector acknowledgement,
or claim native-device or power-cut durability.

## Fixed root and identity boundary

Production code opens only
`/var/lib/hardened-blikvm-diagnostics`, with fixed `journal-a.bin`,
`journal-b.bin`, `.journal-a.tmp`, and `.journal-b.tmp` names. It resolves the
compile-time `hardened-blikvm-broker` account and `hardened-blikvm` group and
requires the current effective UID/GID to match their distinct nonzero IDs.
There is no production path-taking constructor, environment override, shell,
network, logging, collector, acknowledgement, HTTP, or UI surface.

The root must be canonical, mode 0700, owned by the expected UID/GID, free of
POSIX access/default ACLs, and identical by device/inode/owner/group before and
after opening. One no-follow directory descriptor is held under a nonblocking
exclusive `flock` for the store lifetime. Every entry is enumerated against
the four-name allowlist.

Copies and temporary files are accessed relative to the pinned directory with
fixed `openat` names plus `O_NOFOLLOW`, `O_CLOEXEC`, and `O_NONBLOCK`. They must
be same-device regular files with one link, exact owner/group and mode 0600, no
access ACL, and an exact or bounded encrypted-copy size. Symlinks, directories,
FIFOs, hard links, oversized data, unsafe modes/ACLs, unexpected entries, root
replacement, and unsafe abandoned temporaries fail closed.

## Atomicity, entropy, and recovery

Initialization and updates obtain one fresh 24-byte nonce from
`rand_core::OsRng`; entropy failure and the all-zero value fail closed. Tests
alone can inject deterministic entropy. The pure journal rejects retained
active-key nonce reuse and writes only with the active key while reading one
previous key during bounded rotation.

Each copy replacement performs a complete write, flush, file sync, metadata and
ACL validation, authenticated readback, same-directory atomic rename,
directory sync, no-follow reopen, and second authenticated equality check. The
non-selected copy completes first. Both copies are then reselected at the
expected generation and compared to the typed mutation. Immediately before the
first write, the store also rechecks exact selected ciphertext, so an external
same-generation re-encryption is rejected as a concurrent state change.

Repair copies already authenticated bytes and consumes no nonce. A safe corrupt
or truncated copy can be recovered from its authenticated peer. Split brain,
generation gaps, wrong or missing keys, rollback below the caller's floor,
nonce failure/reuse, readback mismatch, and concurrent state changes remain
hard failures.

Eighteen focused tests cover identity/path locking, initialization/update,
same-generation ciphertext races, unsafe inode/metadata/ACL cases, unexpected
entries and temporary recovery, corruption/truncation repair, active/previous
key rotation, nonce/wrong-key/floor failures, plaintext absence, redacted
errors, and all four injected durability return points on both the first and
second copy writes. These tests observe host filesystem behavior after an
injected return; they are not a storage-controller power-loss proof.

## Host verification

Longrun job `572b339464404fb0872e92b416629d6f` exited zero. It passed locked
workspace format/check/test/Clippy, source/packaging/systemd boundaries,
all-feature broker checks, 142 Python tests, compilation and BasedPyright,
production TypeScript/Vite, ESLint, and Prettier. The focused store suite passed
all 18 tests; focused Clippy and rustdoc with warnings denied also passed. LSP
diagnostics reached the configured 100-document limit, so compiler, Clippy,
tests, and direct source-boundary checks are the semantic evidence.

Locked metadata contains 170 packages: 53 workspace and 117 external, with no
missing external license expression. The store adds one workspace package and
no external dependency. A refreshed `cargo audit --deny warnings` loaded 1,233
RustSec advisories and accepted the exact lock graph.

## Independent AArch64 builds

Jobs `5ec7f310694e42fb8ae7e9f95a471c21` and
`f84526bc62174dc28b19382ca3c97bd1` used independent target and bundle roots
under `/mnt/d/debug/blikvm/`. Both completed from the clean source commit above.
Their manifests retain `releaseAccepted=false` and `deviceAccepted=false`.

The byte-identical 4,799,880-byte default prototype archives have SHA-256:

`58b4058352bc54695b58f1384161c64275c4527fe9e44958b9490a1cc738fdc5`

The archive contains neither the store nor journal library and no key, store
root, maintenance tool, credential, diagnostic feature, or persistence unit.

The unshipped 144,530-byte store rlibs were byte-identical:

`d2dbac33ba251e128252c5c09fe9c81615b60ba17f58c38d31a33f3d57fbaca0`

Their 71,558-byte standalone metadata and 71,364-byte ThinLTO member were
byte-identical with SHA-256, respectively:

`ac670606ee50e12bf0f8974a7a9b9aa4814c09c1746ae46b33d3d57c27ee18a2`

`5ea2f0d2c665b5641fb817f9eaad762a8a0fb9fdf88e39ed42a02dc6ec5bda68`

The updated 285,324-byte journal rlibs were byte-identical:

`80c03e7162a6583fc3a3ff6e724fdf264ac0bed97a058076a6d4493785ef7f94`

Their 170,513-byte standalone metadata and 111,844-byte ThinLTO member were
byte-identical with SHA-256, respectively:

`246a736f90e7e8ab2cdceb370db0362f8cff4839966851d8ae330a616b2d3c0d`

`1bef8d5ba87c041248108973b1f95f2569fdae34470dbdfebf81ca7029c7af00`

No device test was run for this source checkpoint. The production root is
unprovisioned and no live process links the crate. The fixed-format systemd
credential loader was subsequently completed at source `b3266c3`; see
`diagnostic-key-credential.md`. A root-only maintenance/native store harness,
production source/rotation ceremony, and external floor remain absent. Live
write-readback-sync-ack coordination remains a later independent checkpoint.
