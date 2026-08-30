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
not by signature verification alone. Serving an older correctly signed bundle
must therefore be rejected before staging/activation.

## Current implementation boundary

`bli-kvm-update-verifier` verifies archive metadata, signature, manifest shape,
required files, hashes, modes, exact tree, and resource limits without writing
payload files. Its tests cover wrong key, signed-manifest tamper, payload
tamper, wrong target, missing required file, unsorted manifest, traversal,
backslash path, duplicate entry, symlink, hardlink, wrong trust domain, and
non-zero trailing data.

Extraction, atomic slot activation, health confirmation, rollback, key
rotation, channel metadata, and system-image updates are separate gates and
remain unavailable.
