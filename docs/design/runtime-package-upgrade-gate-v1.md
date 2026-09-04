# Runtime package upgrade gate v1

## Purpose

This gate upgrades the already provisioned test H616 from the exact inert
`hardened-blikvm-runtime` development package revision 1 to revision 2 without
changing the application account, TLS pair, persistent Nginx configuration,
KVMD credential, or hardware state. Revision 2 removes two transient
`ConditionPathExists=/run/kvmd/...` boot conditions that can permanently skip
enabled read-only services before KVMD publishes its runtime sockets.

The gate is a test-device migration harness, not the production application
updater. It accepts two complete, locally validated packages: the new package
and the exact prior package used for rollback. Package control/data archives,
closed payload allowlists, modes, ownership, absence of maintainer scripts,
source bundle, source commit, builder commit, and SHA-256 are verified before
any device write.

## Preconditions

The live state must match the bounded fail-closed result of the rejected reboot:

- KVMD and both required Unix sockets are available;
- vendor Nginx owns exact public ports 80/443 and Hardened Nginx is disabled;
- the server remains active, while the enabled broker and video proxy are
  inactive with `ConditionResult=no`;
- the exact revision 1 package files are intact;
- the operator account, rendered Nginx files, and referenced vendor TLS pair
  match their accepted hashes;
- package management is idle, `dpkg --audit` is clean, rootfs is read-write,
  and no web-cutover, reboot, or runtime-upgrade guard is present; and
- the control broker remains disabled and no control socket exists.

## Transaction and rollback

Both packages are uploaded into one root-owned mode-0700 directory under
`/var/lib`. Before `dpkg` runs, the gate installs and synchronizes:

- a marker-bound early-boot rollback service before `multi-user.target`;
- a ten-minute rollback timer and one-shot service; and
- a root-only rollback program that accepts only `boot`, `timer`, or
  `explicit` and verifies the old package hash before use.

Failure disables Hardened Nginx, restores vendor Nginx first, disables the
three product services, reinstalls the exact revision 1 package, verifies all
old package files and absence of revision-2-only files, preserves the account
and configuration hashes, and removes only marker-owned transaction files.
If rollback cannot prove its final state, its persistent marker and boot gate
remain for recovery instead of being silently deleted.

## Success criteria

The gate installs revision 2, reloads systemd, starts the broker/video/server,
then transfers public listeners back to Hardened Nginx. Before disarming the
guards it requires:

- the complete new package file hashes/modes and clean `dpkg` state;
- all three product services active and enabled;
- both packaged unit files free of transient KVMD socket conditions;
- vendor Nginx inactive/disabled and Hardened Nginx active/enabled;
- exact wildcard 80/443 ownership, TLS 1.2 and 1.3, matching served
  certificate, closed First Setup, and local health;
- unchanged account, Nginx configuration, Nginx unit, certificate, and key
  hashes; and
- active KVMD with the control boundary still absent.

An independent SSH transaction repeats the final checks after guard cleanup.
No device reboot, target Power/Reset action, KVMD restart, credential output,
or diagnostic-persistence activation is present in this gate. Browser login,
video, and Diagnostics are validated separately before the corrected guarded
software reboot is retried.
