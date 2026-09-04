# Retained runtime identity and credential provisioning checkpoint

On 2026-09-04, the exact retained inert Debian runtime package received its
fixed least-privilege Unix identities and a separate on-device-generated KVMD
credential on the designated test-only H616. No Hardened BliKVM service was
started or enabled, Nginx was not changed, and no update, diagnostic
persistence, hardware action, or physical Power/Reset operation was exposed.

## Fixed source and host acceptance

Policy commit `6ed647f` pinned the complete declarative sysusers source. Its
SHA-256 is:

`8685ad67a137da51a1640db06ef630a4ddaf3d09663640dd081d8a90c1997620`

That hash matches the documentation copy inside retained package
`hardened-blikvm-runtime_0.1.0~dev1_arm64.deb`. The applied set is one shared
group plus five locked, no-home, nologin identities for platform broker, video,
denial-only control broker, server, and future update supervisor. The update
identity receives no process, credential, writable path, or capability here.

Source commit `e60f37c90bcb7d837050b1c22f2c6d541454c0e6` added:

- a read-only provisioning preflight;
- a prepare-only synthetic prediction/rollback gate;
- two-pass dynamic-ID prediction with a pinned second pass;
- persistent root-only account/htpasswd backups;
- per-file before/predicted-after CAS for passwd, group, shadow, gshadow, their
  four backup files, and `.pwd.lock`;
- synchronized early-boot rollback and an independent timed watchdog;
- device-only nonblocking CSPRNG credential generation;
- exact one-line KVMD htpasswd transition and Unix-socket authentication;
- same-filesystem atomic credential-directory publication; and
- an independent read-only final inspector.

Full Longrun job `fff0cf4d8fb043ed90f5895247178727` exited zero on that source
with 228 Python tests plus the complete locked Rust default/all-features,
feature-specific, Clippy, source/systemd, Python compilation, BasedPyright,
frontend build, ESLint, and formatting matrix.

## Native preflight and prepare-only gate

Read-only Longrun job `548af6f081d048a0bf7620f0d4862571` confirmed:

- the exact retained package/tree/modes/capabilities and clean dpkg audit;
- all six identities, the dedicated KVMD account, configuration root, account
  mutation locks, and provisioning rollback artifacts absent;
- the package's fixed sysusers copy and all required tools available;
- kernel CSPRNG readiness, sufficient capacity, free loopback ports, inactive
  and disabled product units, read-write rootfs, and active KVMD services.

Prepare-only Longrun job `7747aac04c5044f59c1a9bbb5cf2a574` then copied the
five protected database inputs, reproduced the same account transition twice
in a synthetic root, generated a credential without emitting it, synchronized
the boot rollback and timed watchdog, and executed the exact rollback before
any live sysusers, htpasswd, or config mutation. The package-only final state
and complete sensitive-stage cleanup passed. Private receipts are mode 0600 at:

- `evidence/private/device-runtime-provision-preflight.json`;
- `evidence/private/device-runtime-provision-prepare.json`.

## Rejected first live attempt and recovery

The first live job `be45ac08a8e9406cb5158cca5d58c2a6` reached the `config`
phase, then rejected the otherwise valid username source because the validator
incorrectly required 24 bytes instead of the fixed account's 22 bytes plus one
newline. Its automatic cleanup used the same invalid length and therefore
correctly reported cleanup unconfirmed instead of claiming restoration.

Recovery discovery found a complete marker-bound state: account databases and
htpasswd at their exact predicted after-state, the exact configuration tree
present, package/KVMD healthy, product units inactive, and boot rollback still
armed. Initial recovery job `c14634f24c104efdbc3b59bdf9356cdb` made no remote
change because its local in-memory upload object lacked a file descriptor.

Commit `320a52c91f735e9681c106054cbd7c3b72f2eb79` fixed the username size as a
computed 23-byte constant, used a real anonymous temporary file for reviewed
rollback transport, and added a permanent recovery path. Recovery job
`a9cb3a05d23f4a0b9ec7a5dcaaee00d5` re-discovered the same bounded state,
replaced only the owned rollback script with hash-verified fixed bytes,
executed it, and independently proved return to the exact retained-package-only
state. No new provisioning mutation was performed during recovery.

Full Longrun job `4fac25c173ea43d2bdb75a31a7f02736` then exited zero on the
fixed commit with 230 Python tests and the complete project matrix.

## Accepted permanent provisioning

The fixed prepare-only job `3dd45ee8ab954ab88792e6eb7baf0d8d` again passed
prediction, watchdog, boot rollback, sensitive cleanup, and package-only final
state. Post-cleanup read-only preflight job
`5210974a13af4c3bbdb27f0d6d059d58` also passed before the retry.

Live Longrun job `e609c6fbcf884862bd0f85579f2ecff4` then completed prepare,
apply, commit, and final inspection. It proved:

- exactly five distinct nonzero service UIDs, five distinct primary GIDs, and
  one separate shared GID without aliases to existing identities;
- the fixed homes, shells, primary groups, and exact shared-group membership;
- all four account databases and five sysusers auxiliary files matched the
  twice-predicted content and metadata;
- one fixed dedicated KVMD account authenticated `/info` through the existing
  Unix socket using a password that never entered host output, argv,
  environment, receipts, source, Git, or Project Memory;
- `/etc/hardened-blikvm` is a closed root-owned mode-0700 directory containing
  only root-owned regular single-link `kvmd-user` and `kvmd-password` sources at
  mode 0400;
- the broker Unix identity cannot read either persistent credential source;
- account-independent package inventory and Nginx content remained unchanged;
- the exact package remained installed, all four unit templates remained
  loaded/disabled/inactive, loopback ports stayed free, dpkg audit stayed clean,
  and KVMD/KVMD Nginx stayed active; and
- the timed watchdog, boot link/unit, sensitive backups, password staging,
  synthetic root, state, marker, and rollback root were removed only after
  final revalidation.

The private mode-0600 receipt is
`evidence/private/device-runtime-provisioned.json`.

Independent read-only Longrun job `290b2dd8acdb4bdb9ce1b98f5840d30b`
opened a new SSH session and repeated the package hash/tree/mode/capability,
identity collision/membership, credential privacy, dedicated KVMD
authentication, inactive-unit, listener, dpkg, rollback-absence, rootfs, and
KVMD checks. Its private mode-0600 receipt is
`evidence/private/device-runtime-provisioned-inspection.json`.

## Remaining boundary

This accepts the retained package, fixed service identities, and dedicated
KVMD credential sources only. Broker, video proxy, server, and denial-only
control broker remain disabled and inactive. Loopback activation, systemd
runtime isolation, Nginx/TLS integration, privacy/resource soak, reboot
persistence, update trust/recovery, persistent diagnostic history, other
browser engines, physical Power/Reset, and release acceptance remain separate
gates.
