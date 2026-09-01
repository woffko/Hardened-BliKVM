# Diagnostic write-before-ack coordinator checkpoint

On 2026-09-01, source commit
`945eb19098639c733f3f630eb41750b7c1f0d936` added the unlinked
`bli-kvm-diagnostic-coordinator`. The exact state and trust boundary is in
`../design/diagnostic-coordinator-v1.md`.

The crate is `no_std`, forbids unsafe code, and has only the diagnostic journal
as a production dependency. It is absent from every server, broker, service,
package, and prototype payload. It performs no filesystem, credential,
collector, network, process, clock, timer, retry, logging, HTTP, or UI I/O.

## Accepted coordination boundary

Every step receives one nonzero generation floor retained outside the
diagnostic filesystem and one fresh typed collector observation. A selected
generation below the floor fails; a selected generation above it permits only
an explicit floor-advance result. The coordinator never advances the caller's
floor itself.

A journal mutation must pass the repository's synchronous persistence protocol
and a separate authenticated re-load. The selected result must be clean, have
the exact planned generation, and equal the complete planned typed journal.
Newly persisted state then stops until the external floor advances and the
caller supplies a fresh collector observation.

An acknowledgement requires that observation to match the durably selected
`pending_ack` entry. It establishes only one in-memory readiness record. A
second immediate authenticated selection must prove the same generation and
pending binding before one linear permit exposes the exact binding to the
transport caller.

The permit is single-use. `NotSent` leaves the entry pending and consumes
readiness; `Mismatch` requests a fresh read without mutation; applied and
no-sealed outcomes become terminal only after verified persistence. Delivery
ambiguity and acknowledgement-time collector unavailability first persist
`ack_ambiguous`. No outcome causes an internal collector or repository retry.

Dropping a permit records an in-process cancelled binding and blocks normal
observation. Recovery persists pending state as ambiguous, accepts an already
ambiguous state only for reread, accepts an authenticated terminal state from
a write that completed before returning an error, and otherwise fails closed.
One-copy interruption remains degraded and requires separately authorized
repair. A full process crash loses only the in-memory marker: the durable
pending entry must be reconciled against a fresh collector read and never
claims an unobserved acknowledgement.

## Host verification

Twenty-six focused Rust tests cover zero-floor rejection; idle/unavailable
reads; new, same, conflicting, absent, and changed snapshots; mandatory floor
advance and second pre-ack selection; applied, no-sealed, mismatch, not-sent,
collector-unavailable, and delivery-ambiguous acknowledgement results; permit
cancellation; already ambiguous and terminal recovery; missing-binding
conflict; failures before, after the first, and after both copy writes; error
redaction; and bounded repository operation counts.

Focused tests, Clippy and rustdoc with warnings denied passed. Full Longrun job
`ba7f0d3bef634d209ede69374a075c3d` exited zero with the complete locked
Rust/all-features/Clippy, source/systemd, 157-Python-test, TypeScript,
BasedPyright, frontend build/lint, and formatting matrix.

The project LSP server was retried for the journal type graph. Workspace symbol
search returned no semantic match, and document symbols failed with
`document limit exceeded: 100/100`; LSP diagnostics are therefore not claimed.
Compiler, test, Clippy, rustdoc, and boundary checks are the authoritative
source evidence for this checkpoint.

The locked graph contains 173 packages: 56 workspace and 117 external, with no
missing external license expression. `cargo audit` loaded 1,233 RustSec
advisories and reported no vulnerability for the 173 locked dependencies.

## Reproducible AArch64 evidence

Longrun jobs `45263cd87c5e43e183f31095f8bb8d8c` and
`35744ff858574808a31ab4addbcf0f77` used independent target and bundle roots.
The byte-identical 4,802,264-byte default prototype archives have SHA-256:

`7ef8812480c939dab37ac3efa479f651837cf86a7c9a3eb07185f86637b3362b`

Both manifests identify the exact clean source commit and retain
`releaseAccepted=false` and `deviceAccepted=false`. The archive contains no
coordinator, journal/store/key-loader/maintenance artifact, diagnostic unit,
credential, or key. All six shipped executable hashes also match between the
two builds.

The complete coordinator artifacts are byte-identical:

- 127,282-byte rlib:
  `9ce24703cf325ee3a5811482c18c53cf6f64739a163d0c0a7d7fb3717425afc3`;
- 124,131-byte standalone rmeta:
  `2decd1f604a7be2db4ead6af0929c98d403e60ec64dac1169deee41dd987e3be`;
- 2,220-byte uninstantiated ThinLTO member:
  `ba40ec77794cb544f024a2dfd47b1b2ac6dbc156a7c5fc945a753d515d3232cc`.

Because the coordinator implementation is generic, the small library object
does not by itself cover monomorphized target code. Two additional independent
`cargo test --release --no-run` AArch64 builds instantiated all 26 test paths.
Their static-PIE 957,664-byte executables are byte-identical:

`6355407a9dfddb2b945e9cd4be064ded39f04fed886e88c0027e4f846ab87e5f`

The cross-built executable was not run on the H616, so this checkpoint accepts
target compilation and reproducibility only, not native device behavior.

## Remaining boundary

No production key or credential source, trusted external rollback floor,
concrete encrypted-store adapter, export-client mapping, live acknowledgement,
broker feature, systemd runtime, HTTP API, Diagnostics tab, package, soak,
reboot, power-cut, or release gate exists. No device or physical Power/Reset
action was performed for this checkpoint. The next step is a separate
default-off concrete store/key/export-client adapter with synthetic crash and
rollback-gated H616 acceptance before any product API or UI is exposed.
