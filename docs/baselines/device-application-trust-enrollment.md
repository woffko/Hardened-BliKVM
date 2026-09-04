# Device application public-trust enrollment checkpoint

On 2026-09-04, the designated test H616 accepted the first reviewed production
application-update public key. The ceremony and device gate never accepted,
uploaded, logged, or stored the signing private key.

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

## Accepted and remaining boundary

This checkpoint accepts one persistent production application public trust
root on the test H616. It does not initialize update state or slots, start or
enable the update runtime, stage or sign a candidate, expose an update API or
GUI, exercise interruption/reboot recovery, establish system-image layout, or
accept a release. Key rotation and revocation still require a versioned policy.
ATX Power/Reset and equivalent attached-target power controls remain excluded.
