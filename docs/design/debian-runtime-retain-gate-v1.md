# Retained inert Debian runtime gate design

Status: host and reversible H616 retained-package acceptance complete; evidence
is recorded in `../baselines/device-debian-runtime-package.md`. This design
authorizes retaining one already validated inert package; it does not authorize
account or credential provisioning, service start/enablement, active Nginx
configuration, update trust, diagnostic persistence, reboot, or hardware
action.

## Exact package boundary

The input must be one absolute regular non-symlink `arm64` package with the
expected SHA-256, product source commit, source-bundle hash, and clean builder
commit. The shared strict parser accepts only the four runtime binaries, the
web tree, four fixed unit templates, and public documentation. It rejects
maintainer scripts, links, special members, privileged mode bits, extended
attributes, files outside the inert payload allowlist, non-root archive owners,
and an unexpected or incomplete manifest.

Installation uses `dpkg -i` directly and never resolves or downloads a
dependency. Preflight requires idle package-manager processes, clean
`dpkg --audit`, sufficient bytes and inodes on `/usr`, `/var`, and `/tmp`, a
read-write rootfs, absent package/production paths/accounts/state and gate
artifacts, free loopback ports, and active existing KVMD services.
`capture_debian_runtime_retain_preflight.py` runs exactly this boundary and
writes only a private allowlisted receipt; it has no mutation path.

## Prepare and commit

The retained gate is deliberately separate from the accepted install/purge
test harness. It reuses only package validation and the strict SSH transport.
Its state machine has two remote phases:

1. Prepare records only private hashes of the account databases, KVMD htpasswd,
   Nginx tree, and all non-target package records. It writes a marker-bound
   rollback root and early-boot unit, creates the exact `sysinit.target` link,
   flushes them with `sync`, and starts an independent five-minute transient
   watchdog before `dpkg -i`.
2. Prepare then verifies every installed regular-file hash, mode, root owner,
   and link count; the exact dedicated directory tree and modes; absent file
   capabilities; all four unit templates loaded but disabled and inactive;
   unchanged account/Nginx/KVMD credential/non-target package state; clean dpkg
   audit; absent accounts/config/state/listeners; and active KVMD services.
3. Commit rereads the marker-owned snapshot and repeats those checks. It removes
   the upload stage before stopping the transient watchdog, then removes the
   early-boot link, unit, scripts, state, and marker. Only the verified inert
   package remains installed.

No product service is started or enabled. No sysusers, tmpfiles, credential,
Nginx, update, diagnostics, or hardware path is invoked.

## Rollback and ambiguous outcomes

The transient watchdog purges the exact package after five minutes unless
commit disarms it. The separately persisted unit runs after local filesystems
and before KVMD/multi-user ordering on an unexpected boot while its exact
marker exists. Both rollback paths validate marker identity and exact package
version/architecture, use `dpkg --purge --force-remove-reinstreq`, and remove
only allowlisted stage/rollback files; there is no broad recursive deletion.

A failure while the device remains reachable invokes the same exact cleanup
and requires the package, production paths, accounts, units, listeners, stage,
and rollback artifacts to be absent while KVMD remains active. A lost response
after rollback was disarmed can leave only an already reverified inert package.
`capture_debian_runtime_retained.py` resolves that outcome through the same
read-only final validator without retrying installation or changing the
device. A still-armed or partial state is not accepted and must first complete
or be recovered; the installer never blindly retries an ambiguous mutation.

## Acceptance and deferred work

Host acceptance requires generated-shell syntax, exact allowlist tests,
BasedPyright, the complete project matrix, and validation of the previously
accepted package bytes. Native acceptance additionally requires a private
mode-0600 retained-install receipt followed by an independent read-only receipt
for the same exact package.

Even successful native acceptance proves only safe package retention. Creating
least-privilege identities and a dedicated KVMD credential, starting the three
read-only services, Nginx/TLS integration, resource/privacy soak, reboot
persistence, update trust/recovery, persistent diagnostic history, physical
Power/Reset, and release remain separate gates.
