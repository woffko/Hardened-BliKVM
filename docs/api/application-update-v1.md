# Signed application bundle v1

Application updates and full-system images use separate formats and trust
roots. This document defines only the application bundle. It cannot write a
kernel, partition, bootloader, device node, absolute path, or arbitrary system
file.

## Container

The artifact is an uncompressed deterministic POSIX tar no larger than
128 MiB. Compression is intentionally outside v1 so gzip concatenation,
trailing-stream, decompression-ratio, and implementation-variance risks do not
enter the first device verifier.

Raw tar iteration is mandatory. PAX/GNU extension records, long-name records,
symlinks, hardlinks, devices, FIFOs, and every type other than regular file or
directory are rejected. Entry paths must be strictly increasing ASCII relative
paths. Backslashes, traversal, repeated dots, empty components, non-UTF-8, and
absolute paths are rejected.

Every entry has UID 0, GID 0, and mtime 0. Directories are mode 0755. Payload
file modes are exactly 0644 or 0755 and must match the manifest. Any non-zero
trailing data is rejected.

The exact tree is:

```text
manifest.json
manifest.sig
payload/
payload/bin/...
payload/libexec/...
payload/share/...
```

The manifest and signature are mode 0644 regular files. `manifest.sig` is
exactly 64 raw Ed25519 signature bytes over the exact `manifest.json` bytes.

## Manifest

`manifest.json` is bounded to 64 KiB, rejects unknown fields, and contains:

- format `hardened-blikvm-application-v1`;
- product `Hardened BliKVM`;
- target `bli-h616-pikvm` and architecture `aarch64`;
- an `app-`-prefixed trust-key ID;
- version and strictly positive anti-rollback sequence;
- strictly positive minimum system sequence;
- exact lowercase 40-hex source commit;
- a strictly sorted, unique file list with relative path, mode, size, and
  lowercase SHA-256.

The manifest must include the Rust server, read broker, video proxy, denial
control broker, and web `index.html`. The archive payload tree must match the
manifest exactly; extra/missing files or directories are rejected. Aggregate
payload is bounded to 256 MiB and 4096 files.

## Trust

Verification uses in-process `ed25519-dalek` strict verification. The trust
root accepts only an `app-` key ID and a valid 32-byte public key. A system
update key cannot be supplied as an application key by naming convention.
Unsigned mode and compatibility fallbacks do not exist.

Sequence downgrade prevention is enforced by the future activation journal,
not by signature verification alone. The low-level stager may retain an older
correctly signed slot, but it cannot select or activate it. The slot manager
must reject any forbidden downgrade before changing the active slot.

## Single-copy staging

`bli-kvm-update-stager` accepts only an absolute regular source file that is
not group- or world-writable. It opens the source with `O_NOFOLLOW`, copies at
most 128 MiB once into a mode-0600 temporary file on the slot filesystem,
flushes and fsyncs it, changes it to mode 0400, reopens the same inode
read-only, and unlinks its last path. Verification and extraction then use
only that read-only descriptor with link count zero. No second read from the
caller-controlled source is possible.

Extraction is a second pass over the already verified immutable descriptor.
It independently checks entry order, type, metadata, exact manifest paths,
sizes, and SHA-256 while streaming into a private mode-0700 staging tree. It
never calls a general tar unpack operation. The original signed manifest and
signature are retained beside a bounded generated receipt, and the completed
slot is reverified from those proofs and all payload hashes.

The slot root must be an existing canonical absolute directory owned by the
stager identity and not group- or world-writable. Temporary files and trees
are created on that filesystem. Files are published read-only (0444 or 0555),
payload directories are 0555, and proof metadata is owner-read-only. The slot
name binds the zero-padded sequence and full manifest SHA-256.

Publication uses Linux `renameat2(RENAME_NOREPLACE)` relative to a pinned slot
root descriptor, followed by directory fsync. There is no overwrite or unsafe
rename fallback. If the deterministic slot already exists, every proof,
directory, file mode, link count, size, and hash is revalidated before an
idempotent `AlreadyPresent` result is returned. A corrupt or conflicting slot
is never repaired or replaced implicitly. The same full validation is exposed
for standalone inspection without retaining the original source archive.

## Current implementation boundary

`bli-kvm-update-verifier` verifies archive metadata, signature, manifest shape,
required files, hashes, modes, exact tree, and resource limits. Its tests cover
wrong key, signed-manifest tamper, payload tamper, wrong target, missing
required file, unsorted manifest, traversal, backslash path, duplicate entry,
symlink, hardlink, wrong trust domain, and non-zero trailing data.

`bli-kvm-update-stager` now implements non-activating, single-copy extraction
and immutable no-replace slot publication. Its tests cover exact staging,
idempotency, corrupt-slot refusal, wrong signatures, payload tamper, oversized
input, writable or symlink sources, unsafe roots, read-only modes, proof
retention, standalone inspection, invalid slot IDs, and residue-free covered
prepublication failures.

The stager is a library and is not connected to the network server, systemd,
or the H616 device. The separate activation state machine is defined in
`application-activation-v1.md`; it remains an unwired library as well.
Clock/boot acquisition, process health credentials, systemd/watchdog wiring,
abandoned staging recovery after process or power loss, slot garbage
collection, key rotation, channel metadata, and system-image updates are
separate gates and remain unavailable.
