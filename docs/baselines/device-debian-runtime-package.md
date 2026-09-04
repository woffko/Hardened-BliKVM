# Inert Debian runtime package checkpoint

On 2026-09-01, the first deterministic Debian package for the read-only
Hardened BliKVM runtime passed host reproducibility and a rollback-gated H616
install/purge transaction. It is deliberately inert: installing it does not
create accounts or credentials, activate Nginx configuration, provision state,
enable or start a service, or include update, diagnostic-persistence, or
hardware-mutation code.

## Read-only device preflight

Source commit `d9edd9e` added a bounded packaging preflight. Longrun job
`e897b72df68448ecb6b62a2c090268f2` confirmed on the designated test-only H616:

- Ubuntu Jammy on AArch64 with `dpkg`, `dpkg-deb`, systemd sysusers/tmpfiles,
  Nginx, and OpenSSL available;
- rootfs read-write and approximately 691 MB free on `/usr` and `/var`;
- the package, all runtime accounts/groups, four unit files, production binary,
  web, configuration, state, slot, and update roots absent;
- loopback ports 18080 and 18443 free;
- `/etc/kvmd/htpasswd` present as a regular mode-0600 file; and
- KVMD and KVMD Nginx active.

The private mode-0600 receipt is
`evidence/private/device-packaging-preflight.json`. It contains only enum and
numeric capacity facts.

## Build-path privacy

The first package build correctly rejected the earlier Rust binaries because
they contained absolute Cargo/Rust toolchain source paths under the host user
home. Product source commit `6b1add8dfe1abf01cf62d606b712e4f7e77a3ec1`
added a Rust compiler wrapper that remaps the exact project root, Cargo source
root, and Rust sysroot while preserving all existing static-PIE rustflags.

`verify-aarch64-elf.sh` now rejects user-home, root-home, mounted-drive, and
Windows-user paths in every shipped ELF. Longrun jobs
`d3e9f393f9714300b33a50fdc0769153` and
`fc544c445d944df7ac095d9afa416b42` used independent target/output roots and
produced byte-identical 4,818,130-byte prototype archives with SHA-256:

`7fbe7d27828f929ba614214860d015b2fb8bed316ea7274828fe1710c88dea21`

All six shipped ELF pairs match and independently pass the path-privacy scan.
Relevant hashes are:

- server: `5f2da39a6563dff23503ec4ebe7c461775b579443dbae3e10c58821d22618fbc`;
- platform broker:
  `7958fd6636b0b26a33cf5aa0fa9429465b01960decf2c618b12a91f695f75157`;
- video proxy:
  `8ed05538a47ed17bd52cc558ddadb886e7fb556858b90739c1654aebf8c71ea0`;
- control broker:
  `becbc9c62ad2f4a270f457453742657425d2edc929f592e1e5bb5218908d8f8e`.

Longrun job `b8c1d2551d484a8a8baddcf664896119` then repeated the full
desktop/mobile Diagnostics TLS Chromium gate with the remapped archive and
completed full rollback.

## Deterministic inert package

Clean builder commit `c0edad8fc26ccbc3199e5714e92860c39c15fcb0` produced two
byte-identical 2,616,688-byte packages:

`hardened-blikvm-runtime_0.1.0~dev1_arm64.deb`

SHA-256:

`53a4d4bfc062057954634e0421a2ec7801e917a488d22c957cd8244d36dba6a5`

The package manifest binds both the remapped product source commit and source
bundle hash, plus the clean builder commit. The control archive contains only
`control`; there are no `preinst`, `postinst`, `prerm`, `postrm`, trigger, or
conffile scripts.

The package installs only:

- the server, platform broker, video proxy, and denied control broker under
  `/usr/libexec/hardened-blikvm`;
- the compiled web tree under `/usr/share/hardened-blikvm/web`;
- four disabled systemd unit templates under `/lib/systemd/system`; and
- public documentation plus Nginx and sysusers examples under
  `/usr/share/doc/hardened-blikvm-runtime`.

It installs nothing under `/etc`, `/usr/lib/sysusers.d`, or tmpfiles paths. The
active sysusers fragment was intentionally moved to documentation so a distro
trigger cannot create accounts during this inert package gate. The package
contains no update supervisor/reporter/unit, diagnostic collector/store/key,
system-action artifact, credential, active Nginx file, or mutating backend.

## Reversible H616 dpkg evidence

Longrun job `20c5ec588bb94ddcb6250d926ea11260` passed the exact package on
the test-only H616 under a five-minute transient systemd rollback watchdog.
The private mode-0600 receipt is
`evidence/private/device-debian-runtime-package.json`.

The accepted gate proved:

- clean dpkg state, absent package/paths/accounts/units, free ports, active KVMD
  services, and read-write rootfs before installation;
- remote package size/hash and local closed control/data/manifest validation;
- successful `dpkg -i` with all 38 installed regular-file hashes, modes, and
  root ownership matching the package;
- all four units remained disabled and inactive;
- no runtime account/group, credential/configuration/state/slot/update root,
  listener, or hardware action appeared;
- `/etc/passwd`, group, shadow, gshadow, the KVMD htpasswd, and the complete
  Nginx file tree remained byte-identical;
- `dpkg --purge` removed the package and all package-owned production paths;
  and
- the rollback unit, staging tree, ports, accounts, and package were absent in
  the final check while KVMD/KVMD Nginx stayed active and rootfs stayed
  read-write.

The standard dpkg audit-log entries from install/purge are intentionally
retained; the gate does not erase or forge package-manager history.

Two earlier preflight attempts made no device mutation. The first lacked phase
detail. The second identified an `accounts` check whose final negative
`getent group` status was accidentally returned as failure. Explicit `if`
branches and `return 0` fixed the harness before the accepted run.

## Final host matrix

Clean evidence source commit
`f32aa80b0e3b2dc09dd901fa1fa7dd052e2c8513` passed complete Longrun job
`14bdd1ce78b946b1b7017355557fb822`. The job exited zero after the locked
default and all-features Rust workspace checks/tests, all required feature
tests, Clippy with warnings denied, source and systemd boundary checks, 191
Python tests, Python compilation, BasedPyright with zero diagnostics, frontend
production build, ESLint with zero warnings, and both Prettier checks.

## Remaining boundary

This accepts the deterministic inert `.deb` and reversible package-manager
transaction. It does not retain the package, provision service accounts or a
dedicated KVMD credential, start/enable services, activate Nginx, install the
application update runtime/trust root, run a long resource soak, reboot,
power-cut, or publish a release.
