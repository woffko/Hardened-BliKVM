# Diagnostic key credential v1 design

Status: design boundary for an unlinked systemd credential loader. This
document does not authorize a production key, persistent source credential,
key generator, rotation ceremony, diagnostic-root provisioning, maintenance
unit, broker linkage, acknowledgement, HTTP, UI, package, or release.

## Systemd 249 delivery contract

The H616 baseline has systemd 249 with plain `LoadCredential=` support and no
`systemd-creds` CLI. The v249 `systemd.exec` contract copies arbitrary binary
credential data into a per-unit read-only location, restricts it to the unit
user and root, uses non-swappable memory when possible, and exports only the
directory path through `CREDENTIALS_DIRECTORY`:

- <https://github.com/systemd/systemd/blob/v249/man/systemd.exec.xml#L2513-L2553>

The key bytes are never placed in the environment. The production loader has
one argument-free entrypoint, obtains the non-secret directory path from that
standard variable, and accepts only these compile-time paths:

- `/run/credentials/hardened-blikvm-diagnostic-store-init.service`;
- `/run/credentials/hardened-blikvm-diagnostic-store-inspect.service`.

Platform Broker is deliberately not accepted until the later live-coordinator
gate grants it key authority.

The only accepted filename is `diagnostic-journal-keyring`. There is no public
path-taking API, fallback directory, command-line option, configuration value,
or direct `/etc` reader. Tests alone inject a temporary directory and expected
identity.

Systemd's runtime copy does not protect the persistent source credential at
rest. Version 249 offers no accepted encrypted-credential ceremony on this
device, and the hardware baseline has no usable TPM, TEE, or RPMB anchor. The
source path, creation method, backup/rotation/recovery policy, and external
rollback floor therefore remain release blockers rather than implicit parts of
this loader.

## Exact 96-byte format

One binary credential always contains both fixed key slots so file length does
not reveal whether rotation is in progress:

| Offset | Bytes | Meaning |
| --- | ---: | --- |
| 0 | 8 | magic `BLIDKEY\0` |
| 8 | 1 | format version `1` |
| 9 | 1 | algorithm `1` = XChaCha20-Poly1305 |
| 10 | 1 | flags: `0` or bit 0 for previous-key presence |
| 11 | 1 | zero reserved |
| 12 | 4 | nonzero active key ID, little-endian |
| 16 | 32 | nonzero active key bytes |
| 48 | 4 | previous key ID, little-endian |
| 52 | 32 | previous key bytes |
| 84 | 12 | zero reserved |

When bit 0 is clear, the entire previous slot must be zero. When it is set, the
previous ID and key must be nonzero and both the ID and key material must differ
from the active slot. Every other flag or nonzero reserved byte is rejected.
There is no text, Base64, JSON, delimiter, newline, KDF, checksum, serializer,
or embedded source path. Corruption that still forms a canonical but different
key safely becomes journal authentication failure; persistent-source integrity
belongs to the later provisioning ceremony.

The journal key-ring constructor independently rejects duplicate IDs and uses a
constant-time comparison to reject duplicate key material under different IDs.
This prevents a nominal rotation from bypassing the journal's per-key-ID nonce
reuse protection. New journal writes still use only the active key.

## Directory and file validation

The loader resolves the exact non-root `hardened-blikvm-broker` account and
`hardened-blikvm` group, then requires the current effective UID/GID to match.
The credential directory must be absolute, canonical, non-symlink and identify
the same device/inode/owner/group/mode/link/length tuple through path metadata
and an `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC` descriptor.

Systemd 249 implements two credential access layouts. The loader accepts only
their canonical forms:

- preferred ACL delivery: root-owned/root-group mode 0500 or 0550 directory
  and mode 0400 or 0440 file, with one exact Linux POSIX access ACL granting
  only the service UID `r-x` or `r--` respectively;
- ramfs/no-ACL fallback: service-UID-owned mode-0500 directory and mode-0400
  file, root or service GID, and no access ACL.

The exact ACL must contain version 2 and only `USER_OBJ`, the resolved service
`USER`, zero `GROUP_OBJ`, matching `MASK`, and zero `OTHER` entries in canonical
order. Default directory ACLs, any extra principal/permission, a foreign owner
or group, and every writable/world-accessible mode fail closed. This matches
the v249 `write_credential()`/`acquire_credentials()` implementation rather
than assuming one filesystem's ACL support:

- <https://github.com/systemd/systemd/blob/v249/src/core/execute.c#L2282-L2441>

The fixed credential name is opened relative to that descriptor with
`O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK`. It must be a same-device,
single-link regular file in one accepted systemd access layout and have exact
length 96. Nonblocking open prevents a substituted FIFO from hanging the
process.

The loader reads exactly 96 bytes plus EOF, rechecks metadata and ACL, reopens
the same fixed name, repeats the complete read, and requires identical inode
identity and constant-time-equal bytes. It then revalidates the directory path
and descriptor. Replacement, same-inode rewrite, truncation, growth, metadata
change, symlink, hard link, directory, FIFO, broad mode, ACL, wrong owner/group,
or path replacement fails with a fixed coarse error.

## Memory and observability

Raw read buffers have a dedicated non-`Debug`, non-`Clone` wrapper and zeroize
on every drop. Decoding also zeroizes its input in place on success and every
normal error. Temporary active/previous arrays are zeroized on rejection. The
returned owner type and underlying journal key types implement neither
`Clone` nor `Debug`; key bytes zeroize on drop. It exposes only non-secret key
IDs and a borrowed typed key ring.

All error variants carry no data. `Display` and `Debug` therefore contain no
path, unit, filename, UID/GID, raw I/O error, key ID, key bytes, credential
bytes, journal bytes, or test location. The crate contains no logging or
printing API.

## Deferred boundaries

The first implementation is an unlinked library. Deferred work includes the
persistent key source and ceremony, encrypted-at-rest or hardware-backed
delivery, external rollback floor, root-only initialize/inspect maintenance
binary, static units/tmpfiles, native H616 credential/store gate, key rotation
recovery, live write-readback-sync-ack coordinator, broker feature linkage,
authenticated HTTP/UI, packaging, privacy review, soak, reboot, and release.
