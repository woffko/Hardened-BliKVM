# Inert application-update runtime package checkpoint

On 2026-09-04, the designated test H616 retained a separate deterministic
application-update runtime package without receiving update authority. The
existing Hardened GUI and read-only runtime were not restarted or changed.

## Package boundary

Source commit `1467021` added
`hardened-blikvm-update-runtime_0.1.0~dev1_arm64.deb`. The package depends on
the exact accepted `hardened-blikvm-runtime (= 0.1.0~dev2)` and contains only:

- the static-PIE Rust update supervisor and health reporter;
- nine disabled update/current/candidate systemd templates;
- the public application update/activation specifications; and
- sysusers/tmpfiles source under documentation examples only.

It contains one control file and no maintainer script, trigger, conffile,
active `/etc` configuration, active sysusers/tmpfiles fragment, public trust
key, private key, update state, slot, runtime, HTTP route, GUI route,
system-image writer, or hardware-control backend.

Direct inspection of the first preliminary packages found that their archive
root inherited mode 0700 from the build umask. Those packages were never used
on the device. Commit `17d8562a9508b87868a1d6b9bd97cd540f579421` made the
common package normalizer set an explicit canonical mode 0755 and added a
regression test.

Longrun jobs `dbcfa9f79e1341c284e4a6aae8002503` and
`5a57864ea1374c40857fb083d346be68` then built the package from independent,
byte-identical AArch64 source bundles. The two final `.deb` files matched
byte-for-byte with SHA-256:

`a42ebcc0ee9e0f4c2e0ad3ca313e783261a41fad3317358da7a0bd0effa9da20`

The manifest binds source bundle
`bc9a6e8a1131b78b86c347a2f38ea6352e89953e3d5002660de8a0438bc5633d`,
product source `531f5f1`, builder `17d8562`, package version/architecture, and
the exact runtime dependency. Control and data archive roots are mode 0755;
the two binaries are mode 0755 and every other file is mode 0644.

## Host gates

The complete host matrix passed at each boundary:

- builder job `f32444243b8e4a268556c6adef3005be`: 303 Python tests;
- canonical-mode job `228b71812afd459f9e0943852dc5620f`: 304 Python tests;
- install-gate job `c99651ea52984e51a130dae0e14e3b28`: 310 Python tests; and
- independent-verification job `2666ac3d103d4bf5aa4df4efb3e59d3a`: 311 Python tests.

Each job also passed the locked default/all-feature Rust checks and tests,
Clippy with warnings denied, systemd/source boundaries, Python compilation,
BasedPyright, the production frontend build, ESLint, and formatting checks.

## Read-only preflight

Commit `a188c4a` added a package-aware install gate with a separate read-only
mode. Job `b454bd0440b84505a4b508c7e491d369` confirmed before mutation:

- the update package and all nine installed unit paths were absent;
- update configuration, trust, state, slots, runtime, and confirmation runtime
  were absent;
- package management was idle and `dpkg --audit` was clean;
- the accepted runtime package and all persistent services were healthy;
- the account, rendered Nginx files, referenced TLS pair, service PIDs, and
  restart counters were captured; and
- no reboot, web-cutover, runtime-upgrade, or update-package guard existed.

The private mode-0600 receipt is
`evidence/private/device-update-runtime-package-preflight.json`. Its policy
records that no package was uploaded and no device state changed.

## Guarded retained installation

Live job `b6fe4337a3b74ceeab867d31d4a0a08c` uploaded only the validated
package under a marker-owned root, armed a synchronized early-boot rollback
and ten-minute timer, installed the package, and retained it only after proving:

- exact package identity, all 16 file hashes/modes/root ownership/single-link
  metadata, and absent file capabilities;
- every update/current/candidate template remained inactive and not enabled;
- update configuration, trust, state, slots, runtime, and authority remained
  absent;
- all existing broker/video/server/Nginx PIDs and restart counters remained
  unchanged;
- the application account, Nginx configuration/unit, and TLS hashes remained
  unchanged;
- the persistent Hardened listener, health, KVMD transition backends, exact
  public ports, and absent control boundary remained healthy; and
- `dpkg --audit` stayed clean and all rollback artifacts were removed.

The rollback path purges only this new package and refuses to remove its marker
until the prior runtime and absence invariants are re-established. It does not
restart KVMD, the Hardened runtime, or the device.

Commit `f33dcb7` added a receipt-bound, read-only `--verify-only` path. Job
`532afbf4c30e45f484bef9aaac751540` opened a new SSH session and repeated the
package, unit, absence, PID/restart, account/configuration/TLS, runtime health,
and guard-cleanup checks without upload or mutation.

Finally, Chromium job `70a4fce9480b4d13890d09f1a04267f0` passed the existing
operator account, inventory, Diagnostics, four MJPEG responses, decoded video,
reconnect, secure cookies, logout, active-stream revocation, zero control
requests, zero WebSockets, and legacy-branding absence.

The accepted private receipts are:

- `evidence/private/device-update-runtime-package.json`;
- `evidence/private/device-update-runtime-package-verification.json`; and
- `evidence/private/device-existing-account-after-update-package.json`.

## Accepted and remaining boundary

This checkpoint accepts deterministic packaging and inert retained installation
of the application-update processes and systemd templates. It does not accept
or create a production public key, signing private key, supervisor
configuration, state/slot roots, enabled unit, staged candidate, update
API/GUI, interrupted write, repeated reboot, system image, recovery path, or
release. ATX Power/Reset and equivalent attached-target power controls remain
excluded.
