# Device application public-trust enrollment checkpoint

On 2026-09-04, the designated test H616 accepted the first reviewed production
application-update public key. The ceremony and device gate never accepted,
uploaded, logged, or stored the signing private key.

On 2026-09-05, that still-unused trust root was retired because its encrypted
private-key passphrase was no longer available. No application state or slot
had been created with it. A separately backed up and reviewed passwordless
owner-only key now supplies the active application public trust root.

## Reviewed public identity

The operator generated and separately backed up an AES-256-encrypted PKCS#8
Ed25519 private key outside the repository, `/tmp`, Project Memory, and the
device. The corresponding closed public-only request was explicitly reviewed
before device contact:

- key ID: `app-production-2026-09-01`;
- public-key SHA-256:
  `7aa74923539f4745f89c96cb823d3ad5bea0e420d7453f4ea5d76191e196832d`;
- exact enrollment-request SHA-256:
  `0c7b995accf0edffe9a463b6b9822a74c079465e663ba93da4e824c170909208`;
  and
- canonical supervisor-config SHA-256:
  `c1def3ce3d7c0bdd9db2bae51088ec7220304bc72065faba824f9802de87830f`.

Commit `3721e3a` added the interactive non-overwriting host generator and five
focused tests. The full Python discovery run then passed 326 tests. The
generator leaves the encrypted private key mode 0400 in an owner-only local
data directory and emits only public review material. An online WSL host is
not treated as offline custody merely because the helper was used.

## Read-only preflight

The gate consumed exact owner-only `/tmp` snapshots of the reviewed request and
the accepted 503,800-byte AArch64 static-PIE validator from source `14a8dc8`.
Both independent validator builds matched SHA-256:

`f468ff8eec21a28e97acb6efc4ed6c2ca2fdcced1f1a778aa9ac9bcc70682071`

Longrun job `047a76b289464458aa7775d918021fb6` passed without changing
the device. It confirmed the exact inert update-runtime package, absent trust
configuration/state/slots/runtime, inactive and not-enabled update units,
healthy persistent services, zero restart counters, unchanged account,
Nginx, and TLS hashes, a root-owned mode-0700 configuration parent, and no
stale enrollment or update guards.

The private mode-0600 receipt is
`evidence/private/device-update-trust-preflight-production-2026-09.json` and
records `deviceChanged=false`, `privateKeyAccepted=false`, no reboot, and no
target power control.

## Guarded enrollment

Longrun job `1ab2797123a24e29a12388955d4a3828` repeated the preflight,
uploaded only the hash-bound public request and transient validator, validated
the request on AArch64, armed synchronized early-boot and ten-minute rollback,
and atomically published only:

`/etc/hardened-blikvm/update-supervisor.json`

The file is root-owned mode 0600 and matches the canonical config hash above.
The gate then removed every marker, timer, service, rollback directory, and
transient stage before opening a new SSH transaction for independent final
verification. That verification proved the configuration persistent, all
staging and guards absent, application state/slots/runtime absent, every
update unit inert, and the existing service PIDs, restart counters, account,
Nginx, TLS, health, and control boundary unchanged.

The accepted private mode-0600 receipt is
`evidence/private/device-update-trust-enrollment-production-2026-09.json`.
Its policy records public-key-only enrollment, no private-key input, no state
initialization, no enabled unit, no runtime restart, no reboot, and no target
power control.

## Unused-root retirement and replacement

Commit `091866a` added an explicit passwordless-key policy. Passwordless
generation requires both `--without-passphrase` and
`--acknowledge-unencrypted-host-key-risk`; signing requires the same policy
acknowledgement. The key remains outside Git, `/tmp`, Project Memory, and the
device as an owner-only mode-0400 PKCS#8 file, but relies only on host/account
protection and is weaker than the encrypted default.

The operator separately backed up and reviewed the replacement public identity:

- key ID: `app-production-2026-09-05-01`;
- public-key SHA-256:
  `e9f50cc5f83f1556fe52bece7301e5743e3106856cfa703c7aa7775be548da41`;
- exact enrollment-request SHA-256:
  `c6cab9580246a4782d3ab8185115423d9ea7870d0be5591c437529d492ac592e`;
  and
- canonical supervisor-config SHA-256:
  `93b2ae14c5dd12d831de01f4a4ba491667a5eee34512e161ef34e7cd29590c5e`.

Commit `14562c2` added the exact unused-trust removal gate documented in
`design/unused-update-trust-removal-gate-v1.md`. It accepts only the prior
completed enrollment receipt and exact config hash while configuration has
never acquired state, slots, runtime, or an active/enabled update unit.

Read-only job `d0e62a9855644cdea374b3e5fa7e030e` first accepted the old
configuration and unchanged runtime. Job `b43ed85a37ec400db6b9cdd6fd47c479`
then performed the single exact config unlink and independently proved the
no-trust state. It did not read, modify, or delete the old encrypted private
key or backup. Its private receipts are:

- `evidence/private/device-unused-update-trust-removal-preflight-2026-09.json`;
  and
- `evidence/private/device-unused-update-trust-removal-2026-09.json`.

One previously opened interactive builder completed late and produced a valid
sequence-one bundle signed by the retired `app-production-2026-09-01` key. It
was never uploaded or staged on the device and cannot validate under the new
trust root. The artifact was retained without modification but moved under the
explicit local staging name
`retired-old-key-app-production-2026-09-01-application-531f5f1-seq1-b354b46d`;
its bundle SHA-256 is
`b354b46d6f991e2c8365a9882c3e4d547edfd6dfc3cd0bd5d6342426b415a887`.
It is evidence only and must never be used as a current candidate.

Read-only job `01224421cb6d4f6bbcc32faf92365251` accepted the clean state and
new public request before guarded enrollment job
`fe51f9cb797741549303023c74b5092b`. The latter repeated the preflight,
validated the public request through the accepted AArch64 validator, armed and
then removed early-boot/timed rollback, atomically published the 372-byte
mode-0600 config, and independently verified it in a new SSH transaction.

The final facts again prove all staging/guards absent, state/slots/runtime
absent, update units inert, and the persistent service PIDs, zero restart
counters, account, Nginx, TLS, health, and control boundary unchanged. The
private receipts are:

- `evidence/private/device-update-trust-preflight-passwordless-2026-09.json`;
  and
- `evidence/private/device-update-trust-enrollment-passwordless-2026-09.json`.

## Accepted and remaining boundary

This checkpoint accepts `app-production-2026-09-05-01` as the current persistent
application public trust root on the test H616. The original unused public root
is no longer active, while its local encrypted key material remains untouched.
This does not initialize update state or slots, start or enable the update
runtime, stage or sign a candidate, expose an update API or GUI, exercise
interruption/reboot recovery, establish system-image layout, or accept a
release. Key rotation and revocation after first use still require a versioned
policy. ATX Power/Reset and equivalent attached-target power controls remain
excluded.
