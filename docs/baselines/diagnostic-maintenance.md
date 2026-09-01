# Diagnostic maintenance and native store checkpoint

On 2026-09-01, source commit
`dfb47a22133d0d43a5318f34fe1d05eb18fd5bfb` added the empty-only
`bli-kvm-diagnostic-maintenance` executable, two static one-shot units, and one
tmpfiles root. Follow-up gate commits through
`961c8fe8ce608e2160ce25dfa9d217a3ddd50c3f` added bounded recovery diagnostics
without changing the tested AArch64 binary, units, tmpfiles source, journal,
key loader, or store.

The exact command and unit boundary is in
`../design/diagnostic-maintenance-v1.md`. The maintenance assets remain
unshipped and absent from the default prototype archive. No production key,
persistent credential source, external rollback floor, live acknowledgement,
broker feature, HTTP route, UI, or package was created.

## Empty-only maintenance boundary

The binary accepts exactly `initialize-empty` or `inspect-empty`, with no path,
key, key ID, floor, generation, repair, update, rotation, acknowledgement,
network, shell, or process-control argument. It loads the fixed systemd
credential and fixed store only through their accepted typed APIs.

`initialize-empty` requires complete copy absence, writes the canonical
encrypted generation one through the atomic A/B protocol, and verifies a clean
zero-entry selection. `inspect-empty` calls only `load_read_only(1)` and accepts
that exact state. Store opening is now nonmutating; a validated crash temporary
returns `RecoveryRequired`, and a degraded copy is reported without cleanup or
repair.

Six maintenance tests cover the exact parser, generation-one initialization,
nonmutating inspection, degraded/nonempty/wrong-generation rejection, backend
failures, and fixed redacted receipts. Nineteen store tests include the new
read-only temporary/degraded invariant. Eleven loader tests include the exact
systemd 249 ACL parser plus the ownership fallback.

## Systemd source boundary

The non-enabled init and inspect units are root-triggered but execute as
`hardened-blikvm-broker:hardened-blikvm`. Both load only
`diagnostic-journal-keyring` from the reviewed persistent-source contract. The
key loader accepts only their two action-specific runtime credential
directories; Platform Broker remains unauthorized.

Both units deny network and device access, carry empty capability sets, use
private mounts/devices/network/tmp, strict filesystem/kernel/proc protection,
and have no `[Install]` section. Init alone receives one exact writable path,
`/var/lib/hardened-blikvm-diagnostics`; inspect receives none. Tmpfiles creates
only that mode-0700 broker-owned/shared-group root and no key or journal copy.
`systemd-analyze verify` and the offline security threshold accepted both
units.

Official v249 source review before the device gate found that systemd prefers a
root-owned credential directory/file with one exact service-UID POSIX ACL, then
falls back to service UID ownership when ramfs lacks ACL support. The loader was
corrected to accept only those two canonical layouts, with 0500/0550 directory,
0400/0440 file, exact ACL entries, no default/extra ACL, pinned descriptors,
two reads, and constant-time byte equality:

- <https://github.com/systemd/systemd/blob/v249/src/core/execute.c#L2282-L2441>

This supersedes the ownership-only native assumption in the earlier
`diagnostic-key-credential.md` checkpoint while retaining its format and
reproducibility evidence.

## Host and reproducible AArch64 evidence

Source host job `1ed44760372449b79e1a1798f051e843` exited zero with the full
Rust/frontend matrix and 144 Python tests. Harness host job
`178db61f86d343ba88317be974db1c73` exited zero with 154 Python tests including
its initial ten rollback/source tests; later bounded recovery changes reached
twelve focused tests and clean BasedPyright. Focused Rust tests, Clippy, rustdoc
with warnings denied, source/systemd boundaries, and a 1,233-advisory RustSec
scan passed.

Final evidence job `914ababb63eb49ee860095b677eee5bd` exited zero from clean
evidence commit `e887653` with 156 Python tests and the complete
Rust/frontend/source/systemd matrix.

Locked metadata contains 172 packages: 55 workspace and 117 external, with no
missing external license expression. Maintenance adds one workspace package
and no external dependency.

Jobs `a10d516c34934158b5313f97f0fc7480` and
`8bc54fad73ed403c9f1b145ebb8b9175` used independent target and bundle roots
under `/mnt/d/debug/blikvm/`. The byte-identical 4,800,673-byte default
prototype archives have SHA-256:

`bc77d8bb555d64306a38d829f0687ee3372c9cd797cf1f35f327e070c834b729`

Their manifests identify exact clean source `dfb47a2` and retain
`releaseAccepted=false` and `deviceAccepted=false`. The archive contains no
maintenance executable/library, key loader, store, journal, diagnostic unit,
tmpfiles root, key, or credential.

The 485,048-byte AArch64 maintenance executables are byte-identical static PIE:

`891b0af33a16d8a32777fb8a53c55a5a043613248fe0c46b86e798fcd1217a95`

The 60,872-byte maintenance rlibs and 46,381-byte standalone metadata match:

- rlib: `b340186b39e513a1b56ccdae6b5c86d40a810a0edbd2d463feb680c819fbd149`;
- rmeta: `5c457eae1776a3ca520ec124e40caf95cb990bfb3fa34f510e9b099954c47acc`;
- 12,832-byte ThinLTO: `d4bdac04dd4e799f796c8f6a88b882e9dc14e6cfd2e9401edd1711c56f05d900`.

The corrected loader pairs also match:

- 108,730-byte rlib: `d0558ee43ee6b32976ccf37d64e3c50b5772b5e0b015b9c9b0c47309a085d15e`;
- 54,540-byte rmeta: `e4f28846c9bcb58685087764eaa4a46b49b5b260bfb0aa012e4dd89648c82871`;
- 52,816-byte ThinLTO: `6842f87cd8a538426cd575bb4478ef4d00af0328ff805fc437c4d4d72ca9bd6b`.

The nonmutating store pairs match:

- 149,704-byte rlib: `ed8c974faaf8f6d3a6204816c77ca5aac3be5b8862e3838114200f7a4a23bfde`;
- 73,805-byte rmeta: `5a879959c1e3429ec0ba9cfa9a8abbad966bbed880a68c56df0c8f3719fab2a6`;
- 74,196-byte ThinLTO: `f8c84dceda6047f095e92b3c9397ae22cec08d1a8facfdb768153c9368dcb38a`.

The journal remains byte-identical:

- 288,470-byte rlib: `d8b7b401869b4909d4666be9d10ec07953de794b8f27478952bcb0db856b75c8`;
- 170,445-byte rmeta: `59262a4ca3bbe928b20977d12c9ffbe1776cdd7ea9962128f74041e44bfc3066`;
- 114,932-byte ThinLTO: `d0d1bc173d30925ed9148d56e7a5bae1f64f2ecc8159871d9e6eff390e68a031`.

## Reversible H616 gate

Longrun job `286a6e60c60d4720964eaf8556a6021b` passed the exact static binary,
units, and tmpfiles source on the designated test-only H616. The private
mode-0600 receipt is `evidence/private/device-diagnostic-maintenance.json`.
It contains only allowlisted semantic facts and public artifact hashes.

The gate:

- began with production root, credential, binary, units, staging, watchdog,
  and test identities absent while rootfs was read-write and KVMD/KVMD Nginx
  were active;
- generated one ephemeral synthetic 96-byte key credential on-device without
  returning its bytes or hash;
- predicted the exact account-database transition before arming a ten-minute
  watchdog, created the temporary broker/shared identities, and later restored
  all four databases byte-for-byte;
- proved real systemd credential delivery was accepted by the corrected loader;
- initialized two equal mode-0600, 16,552-byte encrypted copies at clean empty
  generation one;
- inspected them without changing hashes, inode/size/mtime/ctime metadata, or
  state;
- added a valid crash temporary, proved inspect failed without removing or
  changing it or either copy, then removed only that test temporary and passed
  inspection again;
- invoked no physical Power/Reset, reboot, shutdown, hardware, network, raw
  vendor log, or live acknowledgement action; and
- removed the root, credential, binary, units, stage, watchdog, and temporary
  identities, with KVMD/KVMD Nginx still active and rootfs still read-write.

An earlier attempt failed before phase reporting. Its rollback restored every
persistent path and account database but left a markerless `/tmp` synthetic
root because the allowlist omitted `systemd-sysusers` backup files
(`.pwd.lock`, `passwd-`, `group-`, `shadow-`, `gshadow-`). Bounded recovery
job `06139bc9bc6b48438e1be63aad6dc2a4` removed the final orphan after proving
all persistent paths absent; the recovery receipt is
`evidence/private/device-diagnostic-maintenance-recovery.json`. The allowlist
and idempotent directory cleanup were fixed before the successful replacement
gate.

This accepts native systemd credential metadata and ordinary H616 filesystem
syscall behavior for empty generation one. It does not accept a production key
or source ceremony, encrypted-at-rest provisioning, external rollback floor,
nonempty reconciliation, power-cut durability, live acknowledgement, HTTP/UI,
packaging, reboot persistence, or release.
