# Production application bundle checkpoint

The first application bundle signed by the currently enrolled
`app-production-2026-09-05-01` key was built with the explicitly accepted
passwordless policy. Two runs used the independently built, byte-identical
runtime packages from source `531f5f1e0a818d6c18ec86dbd99ac59f4f8ff4f4`.
Both produced identical 8,499,200-byte uncompressed application archives.

- Bundle SHA-256:
  `d8c144158cc97b0acfd43ebbf7660d0fc1d0a2e8dcc6d378295240564d4ef028`
- Manifest SHA-256:
  `7a088d20f639fabf5ed4f59b7223cd34b8f39cb9fe2c5764acff6df1789fbc6f`
- Source package SHA-256:
  `e9d526f084ba99ba40b8bd324bc1c3712a5ca4dba7f4e9471280d520cd19e74f`
- Enrollment request SHA-256:
  `c6cab9580246a4782d3ab8185115423d9ea7870d0be5591c437529d492ac592e`

The payload contains 27 files totaling 8,468,344 bytes: the accepted server,
three brokers, and complete web tree. Version is `0.1.0-dev2`; application and
minimum system sequences are both one. Source and builder inputs are checked
by the package validator before signing. One rejected preliminary second-run
invocation contained a mistyped source commit; correcting that argument
produced the matching archive above.

## Host verification

`tools/verify_production_update_bundle.py` compares the full bytes of both
builds, binds the reviewed public request and archive hash, and invokes the
real Rust supervisor in fresh mode-0700 directories under `/tmp`. It does not
execute any bundled application binary. The accepted run proved:

- full Rust signature, manifest, tar, and payload validation;
- immutable slot publication and repeat-stage idempotency;
- exact owner, single-link metadata, read-only mode, and content hash for all
  27 payload files;
- generation-one journal with equal active/confirmed slots, floor one, no
  pending operation or previous slot, and matching `current` symlink;
- rejection of repeat initialization, tampered payload bytes, and a different
  valid Ed25519 public key;
- unchanged journal and manager status after the negative checks; and
- absent health challenge and removal of the disposable host roots.

The private receipt is
`evidence/private/production-bundle-new-key-host-verification.json`. The tested
host debug supervisor SHA-256 was
`49e6a173ec55567ba0ad72c1bba0805e43a1768bbd25879034aa4961d172f59e`.
This is host acceptance; the AArch64 service identity and sandbox require a
separate live gate.

The current-key artifacts are under the approved staging root in
`production-application-new-key-531f5f1-seq1-a` and
`production-application-new-key-531f5f1-seq1-b`. Their mounted Windows filesystem
permissions must not be treated as Linux owner-only metadata; device tools
must take hash-verified Linux snapshots before upload. The retired-key archive
remains isolated under its previously documented `retired-old-key` name.

## Next integration issue

Source inspection found a permission mismatch that must be resolved before
starting the packaged update services. Their `User=hardened-blikvm-update`
process directly reads `/etc/hardened-blikvm/update-supervisor.json`, while the
accepted enrollment creates that file root-owned mode 0600 in a root-owned
mode-0700 parent. The unprivileged process cannot traverse/read those paths.
The host debug validation above does not exercise this production ownership
boundary. A delivery mechanism such as systemd credentials needs a reviewed
loader contract and service-identity regression before persistent state/slot
initialization. Do not work around it by running the updater as root or broadly
opening the configuration directory.

The subsequent source change implements `LoadCredential` for activation,
watchdog, and confirmation, retaining the root-owned mode-0600 source. The
loader permits service-owned copies only in `/run/credentials/<unit>/` with
the fixed credential name, mode 0400, private same-owner parent, single link,
root-controlled ancestors and kernel-confirmed read-only file/directory
mounts. `validate-config` performs the load without opening application state.
Rust tests and source/unit checks pass; runtime service-identity acceptance
and a new updater package remain pending. The local systemd regression helper
is `tools/verify_update_config_credential.py`; its attempted execution on WSL
stopped before any fixture/service creation because noninteractive sudo was
unavailable. Use the already authorized test device for the runtime check
after rebuilding the AArch64 supervisor.

The AArch64 runtime check subsequently passed on H616 in Longrun job
`b2dc851408a943ddb82c10d98a2f97eb`. The supervisor built from `edf139f`
has SHA-256
`a6d7c99e1269371a2b1c62aebc2e31ead9ca30b5a1a54356afc367d63804dc59`.
The actual updater identity (UID 984) received exit 11 on direct root-only
config access and exit 0 through the read-only systemd credential. The check
used a synthetic public configuration and transient units with no capabilities,
namespace restrictions, and the system-service syscall filter. Configuration
and state absence checks, credential-mount cleanup, temporary-file cleanup,
and independent unchanged-runtime/trust verification all passed. Receipt:
`evidence/private/device-update-config-credential-edf139f.json`.

This tested a transient binary only. The installed updater is still dev1;
the corrected supervisor and unit templates must be packaged as dev2 and
upgraded with a verified dev1 rollback artifact before state initialization.

That packaging and upgrade step subsequently completed. Two independent dev2
packages from source/builder `e1e8062` matched SHA-256
`b3c385adfbcdbe9d1932982b71a9551f7325f9b8467fc632e194009fc822c4e8`.
Read-only preflight `36adda270d3a44d290116097b9600b1b` passed, then
`a3405e5d032448f6b6e422a83ee7e395` installed dev2 and explicitly restored
dev1, independently verifying all package files, preserved trust, unchanged
runtime, and removed guards. Job `ecb7dd51c1484924ae57cd8b5dc3ec0f` then
retained dev2 with the same final checks. The accepted private receipts are
`device-updater-dev2-upgrade-preflight.json`,
`device-updater-dev2-rollback-rehearsal.json`, and
`device-updater-dev2-retained.json` under `evidence/private/`.

The installed updater is now dev2, with services still inactive and disabled,
trust preserved and state/slots absent. The next rehearsal uses the installed
supervisor to stage and initialize the current signed bundle in disposable
directories under the real updater identity, verifies every payload file
against the installed application, and cleans those directories. Persistent
state initialization and application activation remain separate work.

No new package was uploaded, installed, or activated on H616 during this
checkpoint. Device state/slots remain outside this host verification scope.
