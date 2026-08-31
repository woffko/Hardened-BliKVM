# Atomic system-action store checkpoint

On 2026-08-31, source commit
`83713ad27378682fbd60fb47f64070be829b8541` added the first concrete
filesystem coordinator for the system-action journal. Follow-up commit
`5a764fd540cd689ecc069947649fc1694991d2f4` removed its direct `thiserror`
proc-macro dependency after independent AArch64 builds exposed differing Rust
metadata. The replacement builds below use only the final clean commit.

The `bli-kvm-system-action-store` crate is not linked into any server, broker,
binary, package, systemd unit, API, UI, or actuator. Its production constructor
takes no path and opens only the compile-time root
`/var/lib/hardened-blikvm-system-actions`, with fixed `journal-a.bin` and
`journal-b.bin` names. Host tests use a private `cfg(test)` constructor for a
temporary directory owned by the test UID/GID; that constructor is absent from
production builds.

Opening the production store requires all of the following:

- an absolute canonical root-owned/root-group directory with exact mode 0700;
- no POSIX access or default ACL;
- `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC` open followed by device/inode/owner
  identity comparison;
- a bounded nonblocking exclusive `flock` on the opened directory descriptor;
- no directory entries beyond the two fixed copies and two fixed temporary
  names;
- safe cleanup only for a same-device, single-link, owner/group-matching,
  mode-0600 regular temporary file no larger than one journal copy.

All copy and temporary operations are relative `openat` calls on that locked
directory descriptor. Existing names use `O_NOFOLLOW | O_NONBLOCK`, preventing
symlink traversal and FIFO blocking. Copies must be same-device regular files,
single-linked, owner/group-matching, exact mode 0600, ACL-free, and bounded to
the 2,076-byte journal maximum. A shorter safe file is passed to the journal as
torn content so the other valid copy can be selected and the short copy
repaired. Oversized or metadata-unsafe files are hard failures.

For each replacement the adapter removes only a strictly validated fixed
temporary file, creates a new fixed temporary name with `O_EXCL`, forces mode
0600, writes the complete canonical bytes, flushes and syncs the file, seeks
back and verifies exact content, atomically renames within the same directory,
syncs the directory, reopens the destination, and verifies exact canonical
readback. The non-selected copy completes that entire sequence before the
selected copy is touched.

A process-local mutex serializes calls, while the directory `flock` excludes a
second cooperating process for the store lifetime. Update and repair re-read
and compare the selected state under the lock before their first replacement;
stale expected generations fail without writing. Initialization refuses any
existing copy. A partial initialization is never claimed successful, and one
valid initialized copy can be loaded and repaired instead of reinitialized.

Fifteen focused store tests cover:

- the compile-time production root and fixed filenames;
- absolute/canonical owner/group/mode and POSIX ACL rejection;
- directory-FD lock contention;
- two exact mode-0600 initialization copies and repeat-init refusal;
- successful generation-checked update and stale-generation rejection;
- symlink, directory, FIFO, hardlink, unsafe mode, POSIX ACL, and oversized
  copy rejection;
- unexpected entries, safe abandoned-temp cleanup, and temp-symlink rejection;
- same-size corruption fallback plus non-advancing repair;
- truncated safe-copy fallback plus repair;
- first-copy pre-rename failure preserving the old clean generation;
- second-copy pre-rename failure leaving the new generation repairable;
- post-rename/pre-directory-sync ambiguous failure remaining prepared only;
- failed first initialization write leaving no claimed state;
- external rollback floor and live root-identity replacement rejection;
- post-directory-sync/pre-readback failure recovering the new degraded state.

These are host filesystem tests, not power-cut proof. In particular, the
post-rename test observes the current host filesystem after an injected return;
it does not establish storage-controller or H616 power-loss durability.

The full host suite passed with 91 Python tests plus workspace
format/check/test/clippy, source, packaging and systemd boundaries,
BasedPyright, production TypeScript build, ESLint, and Prettier. Two attempts to
obtain LSP diagnostics for the new store file hit the project server's
100-document limit, so new-file LSP diagnostics are not claimed; compiler and
Clippy checks with warnings denied passed.

The first independent AArch64 pair at source commit `83713ad` produced equal
prototype archives and equal store ThinLTO codegen, but its store rlibs differed
only in `lib.rmeta`. Direct dependency comparison isolated the varying metadata
to `libthiserror`; libc, policy, journal, and store codegen were equal. The
store now implements `Display`, `Error`, and I/O conversion directly. A quick
two-target AArch64 smoke then produced byte-identical store rlibs before the
full replacement matrix was run. The rejected first pair is not used as final
reproducibility evidence.

Two new clean independent AArch64 workspace/bundle builds at final source
commit `5a764fd540cd689ecc069947649fc1694991d2f4` produced byte-identical
4,777,227-byte prototype archives:

`7785c6c7bab5bf5a8051ef391ca4206ef6552b9da67603ff140d607ebc14d2d8`

The unshipped policy rlibs remained byte-identical at 213,992 bytes:

`0621cc3b181fe4ae606ea525c299978c9ddfa4abaa8554f7d299c1e4209a7de3`

Their 175,520-byte AArch64 `lib.rmeta` and 36,664-byte ThinLTO member were
byte-identical with SHA-256, respectively:

`6c452bd889e35aaefa9c882db6e6b3fb8fcb51aa1192bfb7e65f6acac4a8969b`

`ca55d0691e1128778751a5d75530a424faf93b80cf2583ee3ee54370a793be13`

The unshipped journal rlibs were byte-identical at 110,690 bytes:

`d76ff0564ee17e8377cb1f1c68898ceb9149ad79c5b83e3bffbf153e10732364`

Their 71,776-byte AArch64 `lib.rmeta` and 37,784-byte ThinLTO member were
byte-identical with SHA-256, respectively:

`2d66997accbbbaa5aacceca2d09a1bf431e4d7a2fcacda1814a681263c71f9af`

`c76a3d82d04fb71618cbca280fc522521db715125f728ea453987110aa9bef42`

The unshipped store rlibs were byte-identical at 130,380 bytes:

`2553ebe2534a60cc9b7ea38324c3ffeaa24458dfb86069f65214d89fd4f28cbf`

Their 60,960-byte AArch64 `lib.rmeta` and 68,524-byte ThinLTO member were
byte-identical with SHA-256, respectively:

`9a74c5d1a179051708bea3494c95c2734754b9c81cfed4759416fe68c8b140c5`

`9240dee525629caa71bf1921acbace20dbf75c1aa1765f9b978934bc969abffa`

Direct byte comparisons passed for both archives, all three rlib pairs, all
three metadata pairs, and all three codegen pairs. Policy, journal, and store
are absent from the prototype archive. Its manifest identifies the exact final
source commit and retains `releaseAccepted=false` and `deviceAccepted=false`.

No file was created at the production root and nothing was staged on H616. All
filesystem writes were confined to host test/build directories; no device
reboot, shutdown, hostname, service, filesystem, log, clock, Power, or Reset
action was executed. The store remains unavailable until its root provisioning,
external rollback-floor source, systemd confinement, native filesystem and
power-loss matrix, runtime authorization/feedback, fixed actuator, post-boot
acceptance, packaging, and explicit device acceptance pass.

## Native H616 temporary-filesystem gate

The exact 1,006,704-byte AArch64 static-PIE unit harness built from source
commit `b1c3ad2fbcf7c6f0f2f7d5a81aac9e6833fa0616` has SHA-256:

`8e9206d9475ee0f3a7e23f6a59a26a6e7bd84ac9fd70f8e47b673763bb28c6d3`

The marker-protected staging tool was committed at `f404d03` and hardened for
hashed known-hosts plus strict local snapshots at `ef7f9de`. Its first
invocation stopped during local validation before SSH because the Windows build
mount exposed unsafe POSIX mode bits and the enrolled known-hosts entries were
hashed. No receipt or remote stage was created. The successful invocation used
a mode-0555 local `/tmp` snapshot whose filename was bound to the exact hash;
StrictHostKeyChecking still used the enrolled private known-hosts file.

The H616 run passed all fifteen store tests with zero failures and zero ignored
tests. This natively exercised directory-FD locking, POSIX ACL rejection,
special-file and hardlink rejection, canonical and truncated-copy repair,
atomic rename, file/directory sync, exact readback, rollback floor, root
identity replacement, and injected write/rename/sync windows. Test-created
files were restricted to the harness `TMPDIR` beneath its unique `/tmp` stage.

Before, after, and during the separate final-state check:

- the production store root was absent;
- the root filesystem remained read-write;
- the existing KVMD and KVMD Nginx services remained active;
- no rootfs helper or device action was invoked;
- cleanup was confirmed and the remote stage was absent.

The private allowlisted receipt is mode 0600, Git-ignored, and contains no
routing, credential, hostname, address, serial, or other device identity. The
local hash-named harness snapshot was also removed after receipt verification.

This accepts the code path only on the device's executable temporary
filesystem. It does not prove the uncreated production root, an external
rollback floor, systemd confinement, storage-controller persistence, sudden
power loss, reboot/shutdown execution, or post-boot recovery.

A subsequent unshipped maintenance CLI plus static tmpfiles/systemd templates
now constrain provisioning to empty generation one and read-only empty-state
inspection; see `system-action-maintenance.md`. They are host-verified and
AArch64-reproducible but have not yet created the production root on H616.
