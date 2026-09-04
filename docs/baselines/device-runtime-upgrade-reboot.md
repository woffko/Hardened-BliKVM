# Persistent runtime upgrade and reboot checkpoint

On 2026-09-04, the persistent Hardened BliKVM runtime on the designated test
H616 passed a package upgrade, a real software reboot, post-boot Chromium
operation, and an independent final SSH audit. The target-computer Power and
Reset paths remained excluded throughout.

## Rejected reboot and root cause

The first guarded reboot correctly failed closed. The device rebooted and SSH
returned, but the enabled platform broker and video proxy were inactive with
`ConditionResult=no`, `Result=success`, exit status zero, and zero restarts.
KVMD was active and both of its Unix sockets existed after startup. The
four-minute guard restored vendor Nginx without changing the application
account, package, credentials, or KVMD backend.

The package unit templates conditioned startup on KVMD paths under `/run`.
Although both units were ordered after and required `kvmd.service`, KVMD could
publish its sockets just after systemd evaluated the one-shot conditions.
Skipped conditions are not service failures and therefore do not trigger
`Restart=on-failure`.

Source commit `531f5f1e0a818d6c18ec86dbd99ac59f4f8ff4f4` removed only those
two transient path conditions. The Rust processes already create their own
restricted listeners before connecting to KVMD for an individual request, so
a missing upstream still fails closed at the request boundary. KVMD
`Requires`/`After` ordering, identities, credentials, sandbox policy, socket
paths, and restart policy remain unchanged.

The final bounded failure and recovery evidence is stored privately in:

- `evidence/private/device-persistent-reboot-after-orphan-cleanup.json`;
- `evidence/private/device-post-reboot-unit-diagnostics.json`; and
- `evidence/private/device-post-reboot-guard-complete-audit.json`.

## Reproducible revision 2 package

Two builds used independent target and output roots below the approved mounted
debug directory. Longrun jobs `990634d8356e4e8d9bebe0b604693c7e` and
`8f1b08ce6a144e21a2d2da8891347702` produced byte-identical prototype bundles
with SHA-256:

`bc9a6e8a1131b78b86c347a2f38ea6352e89953e3d5002660de8a0438bc5633d`

Two independently constructed `hardened-blikvm-runtime_0.1.0~dev2_arm64.deb`
files were also byte-identical, with SHA-256:

`e9d526f084ba99ba40b8bd324bc1c3712a5ca4dba7f4e9471280d520cd19e74f`

The package manifest binds the bundle, source, and builder to `531f5f1`. Direct
archive inspection confirmed the expected package/version/architecture,
unchanged KVMD ordering and sandbox policy, and absence of both transient
socket conditions.

## Guarded live upgrade

Commits `1256ec8` and `b069319` added a package-aware upgrade gate and its
separate read-only mode. The gate validates both complete packages, including
the exact previously installed revision 1 package, before device mutation.
It stages both packages under one root-owned directory, arms an early-boot
rollback plus a ten-minute timer, preserves the application account/TLS/Nginx
hashes, and can restore the exact old package and vendor listener.

Full host job `ef0050b7d5a44f5c8b18fc02a21e32d0` passed the Rust, Python,
systemd, type, frontend, lint, and formatting matrix with 296 Python tests.
Read-only device job `2f049b337f5e4b3082a23831ffb7f9ff` then accepted the exact
revision 1 state, confirmed the boot race, and returned before upload or any
device change.

Live job `7e6ad13e24844f04abbaf16f2ebe9a4e` upgraded to revision 2 and
restored the Hardened listener. It proved the complete new package, active and
enabled broker/video/server/Nginx, disabled vendor Nginx, exact 80/443 scope,
TLS 1.2/1.3, unchanged account and configuration, absent control boundary,
and complete guard cleanup. The private receipts are:

- `evidence/private/device-runtime-package-upgrade-preflight.json`; and
- `evidence/private/device-runtime-package-upgrade.json`.

## Reboot and browser acceptance

Commit `79ff492` bound the reboot gate itself to revision 2 and to the absence
of both transient socket conditions. Full host job
`d5a115a89df94e04beed9ff816b15f3c` passed before device use. The final
pre-reboot audit found revision 2, all product units active/enabled, zero
service restarts, zero video connections, disabled vendor Nginx, and no reboot
or web-cutover artifacts.

Longrun job `17e02a8c01cf43a390528faf90e8aba8` completed the guarded software
reboot. SSH and HTTPS disappeared, the boot identity changed, and SSH returned
41.0 seconds after the down transition. Before the four-minute guard was
disarmed, the gate proved revision 2, active/enabled services, exact listeners,
closed First Setup, unchanged account, TLS 1.2/1.3, active KVMD, absent control
boundary, and zero web-cutover orphans. The guard was then removed and the same
state was rechecked.

The existing-account Chromium harness was refined to distinguish response from
frame decode and to follow the last active MJPEG response across React-driven
reconnects. At source commit `9350739`, post-reboot browser job
`a32930d6eeb640af933726b794fed65b` passed direct HTTPS login, inventory,
Diagnostics, four MJPEG responses, decoded video, reconnect, secure cookies,
logout, active-stream revocation, zero control requests, zero WebSockets, and
absence of legacy branding.

Independent final SSH job `76a9adbae90f4ecaa3f1f7a86b050de5` then confirmed
revision 2, all runtime units active/enabled with zero restarts, zero retained
video connections, active KVMD and sockets, disabled vendor Nginx, preserved
account/configuration, and absent reboot/web-cutover artifacts. The private
receipts are:

- `evidence/private/device-persistent-reboot-dev2.json`;
- `evidence/private/device-existing-account-video-after-reboot-dev2-2.json`;
  and
- `evidence/private/device-post-reboot-dev2-final-audit.json`.

## Accepted and remaining boundary

This accepts one clean persistent package upgrade and one guarded software
reboot of the test BliKVM device, including post-boot browser video and
Diagnostics. It does not accept a production updater installation or signing
key, repeated/interrupted reboot and power-cut matrices, a production system
image, H616 A/B recovery, other browser engines, a release, or any physical
Power/Reset action. Hardware-mutating HID, ATX, virtual-media, fan, display,
button, and serial adapters remain outside this checkpoint.
