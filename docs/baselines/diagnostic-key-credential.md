# Diagnostic systemd key credential checkpoint

On 2026-09-01, source commit
`b3266c3337fd4945611b1cb4e56b235cf76454f1` added the unlinked
`bli-kvm-diagnostic-key-loader`. It is the only production parser for the
diagnostic journal key-ring credential. No server, broker feature, collector,
store caller, binary, systemd unit, package, API, UI, or prototype artifact
links it.

The exact 96-byte format and threat boundary are in
`../design/diagnostic-key-credential-v1.md`. This checkpoint uses no real or
test-device key and does not create a persistent credential source, production
root, maintenance tool, rotation ceremony, or external rollback floor.

## Systemd and path boundary

The loader follows the target's systemd 249 `LoadCredential=` interface. That
version exposes a read-only per-unit credential copy, restricts it to the unit
user and root, uses non-swappable memory when possible, and exports only the
directory path through `CREDENTIALS_DIRECTORY`:

- <https://github.com/systemd/systemd/blob/v249/man/systemd.exec.xml#L2513-L2553>

Key bytes never enter the environment. The public loader takes no path or
configuration argument and accepts only the standard environment directory
when it equals one of two compile-time unit paths: Platform Broker or the later
diagnostic maintenance unit. The credential name is fixed as
`diagnostic-journal-keyring`; direct `/etc` reads and fallback paths do not
exist.

The source checkpoint resolved the exact non-root broker UID/shared GID and
validated a canonical service-owned mode-0500/0700 directory through path and
pinned `O_DIRECTORY | O_NOFOLLOW` descriptor metadata. It accepted only a
same-device, single-link, service-owned/group-owned mode-0400 file and rejected
all POSIX ACLs.

That host model was incomplete for native systemd 249. The official v249
implementation prefers root-owned credentials with an exact ACL for the
service UID, then falls back to service ownership only when the backing
filesystem lacks ACL support:

- <https://github.com/systemd/systemd/blob/v249/src/core/execute.c#L2282-L2441>

This was found before any device gate or production key. Therefore commits
`b3266c3`/`850495d` remain valid host-format/reproducibility evidence but are
not accepted as native credential-delivery evidence. The subsequent
maintenance checkpoint replaces the access validator with exact support for
both systemd layouts, narrows the unit allowlist to action-specific init and
inspect services, and must pass the H616 gate before the loader is considered
native-compatible.

Two complete reads through fresh descriptors must have identical inode
identity and constant-time-equal bytes; the directory is revalidated after the
second read. Symlink, directory, FIFO, hard link, broad mode, wrong identity,
short/long file, ACL, path/inode replacement, and same-inode rewrite all fail
with fixed coarse errors.

## Canonical key boundary

The fixed envelope carries version/algorithm/flags, one nonzero active
ID/32-byte key, a fixed optional previous slot, and zero reserved bytes. An
absent previous slot must be all zero. A present previous slot must have a
nonzero ID/key, and both its ID and key material must differ from active. No
text, Base64, JSON, newline, delimiter, password KDF, serializer, generator, or
source path is accepted.

The journal key-ring constructor was independently hardened to reject equal key
material under different IDs with a constant-time comparison. This prevents an
ID-only pseudo-rotation from bypassing retained per-key-ID nonce checks. The
credential has a stable synthetic format vector with SHA-256:

`109694761f8f6681004299800682ad088f3eeacb1472d2a493e1fcd0bbd5ba29`

Raw credential buffers implement neither `Clone` nor `Debug`, zeroize on drop,
and are also zeroized in place after decoding. Temporary key arrays are cleared
on rejection. Returned key owners and the underlying journal key types
implement neither `Clone` nor `Debug`; only non-secret key IDs and a borrowed
typed key ring are exposed. Errors contain no path, filename, unit, numeric
identity, raw I/O detail, key ID/material, credential bytes, or journal bytes.

Ten focused loader tests cover the stable vector, active-only and rotating
key-ring interoperability, every canonical format field/relationship,
directory/file metadata and ACL denial, symlink/directory/FIFO/hard-link and
length cases, inode and same-inode races, input-buffer zeroization, fixed error
surfaces, and the two-unit allowlist. The journal's twelve tests also pass with
duplicate-key-material rejection.

## Host verification

Longrun job `1967494749074f289a86bc03e3f8cc2b` exited zero. It passed locked
workspace format/check/test/Clippy, all-feature broker checks,
source/packaging/systemd boundaries, 143 Python tests, compilation and
BasedPyright, production TypeScript/Vite, ESLint, and Prettier. Focused Clippy
and rustdoc with warnings denied passed. LSP diagnostics for both changed Rust
files reached the configured 100-document limit, so compiler, Clippy, tests,
and direct source-boundary checks are the semantic evidence.

Locked metadata contains 171 packages: 54 workspace and 117 external, with no
missing external license expression. The loader adds one workspace package and
no external dependency; libc, nix, SHA-256 tests, subtle, tempfile, and zeroize
were already locked. A 1,233-advisory RustSec scan accepted the exact graph.

## Independent AArch64 builds

Jobs `e481a3873d1f4b36aca821ff496f1c1c` and
`4d09d4f8f23a486f9c2dbc3e5c1764b1` used separate target and bundle roots under
`/mnt/d/debug/blikvm/`. The byte-identical 4,800,533-byte default prototype
archives have SHA-256:

`0f2b1c0886ddf08d497300e28e0da5e81900a7c95812aef5a504587c84ed2484`

Their manifests identify the exact clean source commit above and retain
`releaseAccepted=false` and `deviceAccepted=false`. The archive contains no
loader, store, journal, key, credential, maintenance tool, persistence unit, or
diagnostic feature.

The unshipped 96,390-byte key-loader rlibs were byte-identical:

`e860c815b3daaa4d829018ed61f89d57801741d97660fbb85a5f88b81cbe5659`

Their 45,906-byte standalone metadata and 49,108-byte ThinLTO member were
byte-identical with SHA-256, respectively:

`083608487c0a5abf233327fca6b3ce10783601ead60fb95065ba0aa0139d90b3`

`2b15db63e4f1e413a1004045588fc15ff757ba22b6476b7be6858b425f973f2f`

The hardened 288,470-byte journal rlibs were byte-identical:

`d8b7b401869b4909d4666be9d10ec07953de794b8f27478952bcb0db856b75c8`

Their 170,445-byte standalone metadata and 114,932-byte ThinLTO member were
byte-identical with SHA-256, respectively:

`59262a4ca3bbe928b20977d12c9ffbe1776cdd7ea9962128f74041e44bfc3066`

`d0d1bc173d30925ed9148d56e7a5bae1f64f2ecc8159871d9e6eff390e68a031`

No device test was run for this source checkpoint because no executable or unit
links the loader and no credential was provisioned. Systemd 249 on the H616 has
plain `LoadCredential=` but no accepted encrypted-at-rest source ceremony, and
the hardware baseline has no usable TPM/TEE/RPMB anchor. The next checkpoint is
a root-only initialize-empty/inspect-empty maintenance binary plus static
unit/tmpfiles candidates and a synthetic rollback-gated H616 test. It must use
only ephemeral test key material, restore original absence, preserve KVMD and
KVMD Nginx, keep rootfs read-write, and never invoke physical Power or Reset.

That replacement checkpoint passed at source `dfb47a2` and live job
`286a6e60c60d4720964eaf8556a6021b`. It corrected the systemd ACL/fallback
model, narrowed authorization to the two maintenance units, accepted real H616
credential delivery, and restored all state. Exact updated loader artifacts and
remaining production-key limits are in `diagnostic-maintenance.md`.
