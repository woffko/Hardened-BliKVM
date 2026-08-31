# Reversible H616 diagnostic collector checkpoint

On 2026-08-31, the unshipped diagnostic collector process and its reversible
H616 gate passed on the designated test-only BliKVM v4. Collector source commit
`632c081afcaacdbd7c491e29cdd7bd5ab339c22a` added the fixed-path process
boundary. Gate source commit `9997c5b58bed38ef8a8b8146b02e94bdfc501d0b`
added the first marker/watchdog harness; final gate commit
`28d189a04a7fab3e2b11c1e719b01be0188846cb` added the accepted reversible
account strategy. The intervening commits changed only the gate tool and tests;
the Rust, Cargo, packaging, unit, and sysusers sources used for the AArch64
binary remained unchanged.

The collector owns exactly one fixed socket:
`/run/hardened-blikvm-diagnostics/collector.sock`. It validates an exact
mode-0750 runtime directory owned by its process identity, refuses an existing
socket, binds with mode 0660, and removes only the same socket inode on graceful
exit. The binary accepts no argv, environment, network address, log path, raw
message, or arbitrary configuration. Safe signal handling converts SIGTERM or
SIGINT into the listener library's bounded shutdown path.

The unshipped service template has no `[Install]` section. It runs as
`hardened-blikvm-diagnostics:hardened-blikvm`, retains `DynamicUser=yes` so a
future static or supported dynamic identity is accepted, creates only the
fixed runtime directory, uses a private network namespace with AF_UNIX only,
has no capabilities or device access, and applies the existing strict systemd
hardening set. The separate sysusers fragment and service are intentionally
excluded from the prototype bundle.

## Host and AArch64 verification

The source host matrix passed with 133 Python tests plus locked workspace
format/check/test/Clippy, source and systemd boundaries, Python compilation,
BasedPyright, production TypeScript build, ESLint, and Prettier. The collector
crate's three focused tests cover exact socket mode and cleanup, refusal to
delete a replaced non-socket, and rejection of unsafe/symlink/existing paths.
Ten gate tests cover assets, generated client modes, POSIX scripts, marker and
hash rollback, account restoration, exact receipts, and the physical-action
exclusion. LSP diagnostics are not claimed because the project server remained
at its 100-document limit; compiler and Clippy checks passed.

Two clean independent AArch64 builds at gate source `9997c5b` produced
byte-identical 4,780,314-byte prototype archives:

`81ce25b0eb832fe3b52333c3e0e88259248eab0f3225f3fb97995f339aee0812`

The archives contained no collector executable, collector service, or
diagnostics sysusers fragment. Their manifest retained
`releaseAccepted=false` and `deviceAccepted=false`; all six shipped binaries
and the web tree were identical between builds.

The separate unshipped collector binaries were byte-identical 666,032-byte
AArch64 static PIE executables:

`4e28024e7143e94054a4584e7c76a2430a61cf5eb6bb08792b42fbcd67fd2fcc`

The collector rlibs, metadata, and ThinLTO members were byte-identical:

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| collector rlib | 94,548 | `0bcea9c59d19c8ea9a8fa9a36ac769be973d69b2fd185bda6e6296a66215c944` |
| collector `lib.rmeta` | 39,848 | `ebc85c74ff6cda117290f11a8258e2bd2f7de7a2679f94a32ea5c8b0213df36b` |
| collector ThinLTO | 53,212 | `3ff3976b57af5a44052a0fd611f02f6dc125a07631060278543717e9671f4821` |
| signal-hook rlib | 305,072 | `13cb239cd4dabb0cdd5adba6b77f6daa26be7151be9df65fbf80540b8ec5e502` |
| signal-hook-registry rlib | 124,912 | `d235478502ee741dbfa16f38f4289872bfc1975e3e5e79ff23ac2704a690b8e1` |

The listener/adapter and every lower diagnostic layer, `nix`, `rand_core`, both
workspace `getrandom` variants, signal-hook dependencies, and every archive
member also matched directly. Locked metadata contains 155 packages: 47
workspace and 108 external, with no missing external license expression.
`cargo audit --deny warnings` loaded 1,233 advisories and accepted the exact
lock graph.

## Account-strategy finding

A read-only preflight found the shared group, five producer accounts, and
collector account absent on the current H616. An initial DynamicUser-only gate
proved that the anchor unit could start, but `getent` could not resolve its
identity (`identity-anchor-nss`). The fixed-name Rust resolver therefore cannot
use DynamicUser on this userspace without working `nss-systemd`. No raw journal
or process log was read.

That failed approach restored all external state. An early cleanup edge left
only its marker-owned staging directory and inactive-bound rollback timer after
the binary, unit, runtime directory, and identities were already absent; a
marker-gated recovery removed those final two items while KVMD stayed active.
The rollback was then made idempotent for pre-mutation failure, and bounded
failure-phase receipts were added.

The accepted gate temporarily applied the two exact sysusers templates. Before
mutation it copied `/etc/passwd`, `/etc/group`, `/etc/shadow`, and
`/etc/gshadow` with metadata into a root-only stage and recorded their hashes.
A ten-minute rollback watchdog was armed before `systemd-sysusers` ran. Restore
was permitted only while every current file matched its expected post-sysusers
hash; an unknown concurrent change would fail closed instead of being
overwritten. Each restored file then matched its backup byte-for-byte and by
pre-test hash.

## Native H616 result

The exact AArch64 collector and unshipped unit passed the following live checks:

- all five fixed producer identities received the exact accepted response;
- root, an unknown non-root UID, and a duplicate source were rejected before
  unauthorized event acceptance;
- a partial frame timed out within the fixed bound and received no response;
- a malformed complete frame received no response and terminated;
- an acknowledged live connection observed EOF on graceful service shutdown;
- the collector had no effective or bounding capabilities, had
  `NoNewPrivileges`, used a private network namespace, and exposed only its
  mode-0660 socket owned by the collector and shared group;
- the static sysusers identities, binary, runtime unit, runtime directory,
  socket, watchdog, and staging root were all removed;
- the four account database files returned to their exact pre-test contents;
- KVMD and KVMD Nginx remained active with unchanged process instances; and
- an independent read-only final bitmask returned zero.

The final receipt also confirms the root filesystem remained in its original
read-write state. No raw log, device node, HID, ATX, GPIO, media mutation,
update, reboot, physical Power, or physical Reset action was used.

This accepts only the isolated local collector process and account/systemd
boundary. It does not enable or package diagnostics, connect real producers,
persist bundles, add an authenticated server export, expose UI, or claim
privacy/browser/soak/reboot acceptance. Those remain later gates.
