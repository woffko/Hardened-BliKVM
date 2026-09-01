# Diagnostic persistence adapter checkpoint

On 2026-09-01, source commit
`57959b04010cc6b4bcf01ea1fb861538b63aa4c8` added the unlinked
`bli-kvm-diagnostic-persistence` adapter. Its exact trust and cancellation
boundary is in `../design/diagnostic-persistence-v1.md`.

The adapter adds no executable, unit, package, broker feature, HTTP route, UI,
credential source, floor source, path, socket option, timeout option, retry
policy, task spawn, timer, or logging surface. It is absent from the default
prototype payload.

## Accepted concrete composition

The only public runtime constructor consumes an owned
`LoadedDiagnosticJournalKeys` and `DiagnosticJournalStore`; it cannot load a
credential, open a store, accept raw key material, or select a path. It creates
only `DiagnosticExportClient::new`, retaining that client's fixed Unix socket
and bounded timeouts.

The repository wrapper uses `load_read_only`, so startup cannot clean a crash
temporary or repair a degraded copy implicitly. Updates use the store's exact
atomic method and verify the returned generation before the coordinator's
separate authenticated re-selection and full typed-state comparison.

One explicit async `poll` performs at most one collector read and one
acknowledgement. New sealed state always persists and requests external floor
advance before any acknowledgement. A subsequent call must use the advanced
floor and performs a fresh read plus the coordinator's second immediate
selection before obtaining one linear permit.

Read not-sent/retryable results return without store access or internal retry.
Acknowledgement not-sent remains pending; mismatch requests a later explicit
read; response-level collector unavailability and every delivery-ambiguous
error first persist ambiguity. Cancellation during read is nonmutating.
Cancellation while awaiting acknowledgement drops the permit and makes the
next poll recover without reading. One-copy interruption requires repair;
two-copy completion followed by an error is discovered as terminal without
repeating the acknowledgement.

## Host verification

Nineteen focused Rust tests cover new/pending/absent sealed state; external
floor lag; exact applied/no-sealed/mismatch/not-sent/unavailable/ambiguous
mapping; read and acknowledgement cancellation; no hidden retry; store failures
before, after the first, and after both copies; terminal discovery; bounded
errors; and fake/exact-client future-size limits.

Focused tests, Clippy and rustdoc with warnings denied passed. Full Longrun job
`266e64552ef34f93bc07ff7e1646b1d6` exited zero with the complete locked
Rust/all-features/Clippy, source/systemd, 158-Python-test, TypeScript,
BasedPyright, frontend build/lint, and formatting matrix. An earlier job stopped
only at the formatter before running the matrix; the formatted replacement is
the accepted result.

The project LSP server was retried for the new Rust source and returned
`document limit exceeded: 100/100`; LSP diagnostics are not claimed. Compiler,
tests, Clippy, rustdoc, and source-boundary checks are the authoritative source
evidence.

The locked graph contains 174 packages: 57 workspace and 117 external, with no
missing external license expression. The adapter adds one workspace package
and no external dependency. `cargo audit` loaded 1,233 RustSec advisories and
reported no vulnerability in the 174 locked dependencies.

## Reproducible AArch64 evidence

Longrun jobs `114474ef7fbb4eb19df6ac654f2abb27` and
`edd6e6946c464502b534bcab374a0d85` used independent target and bundle roots.
The byte-identical 4,802,698-byte default prototype archives have SHA-256:

`06164420cfaad73eebc854751e3830386f32c48b34275c2d3a91654b0815922f`

Both manifests identify the exact clean source commit and retain
`releaseAccepted=false` and `deviceAccepted=false`. The archive contains no
persistence/coordinator/store/key-loader/maintenance library or executable,
diagnostic unit, credential, or key. All six shipped executable hashes match
between builds.

The complete persistence library artifacts are byte-identical:

- 101,556-byte rlib:
  `a49044a750525a2669c5ef83e09a0516a0dc2c4c263efea9a2413c55a6f42084`;
- 92,232-byte standalone rmeta:
  `89a2b4e72d136b0099a6c52c098be693018786fd9109c570534c3ed4cd18b1bd`;
- 7,828-byte ThinLTO member:
  `436186a67b44abb481d202668bb2e7ae3cdb76f57424286f82d1a4156101ca05`.

Two additional independent `cargo test --release --no-run` builds instantiate
all 19 async state, cancellation, and fault paths. Their static-PIE
1,129,920-byte AArch64 executables are byte-identical:

`32bcbf8df703cc9ed0efb44942ca35eb21b346fcd407524e387d9e4680e2316c`

The cross-built executable has not yet run on H616. This checkpoint therefore
accepts host behavior and reproducible target codegen, not native device or
concrete fixed-store/socket behavior.

## Remaining boundary

There is no trusted external production floor, persistent key custody,
adapter executable/unit, live store/export-client run, broker linkage,
authenticated diagnostic HTTP API, Diagnostics tab, package, privacy review,
soak, reboot, power-cut, or release gate. No device or physical Power/Reset
action was performed for this checkpoint. The next gate is a separately
reviewed synthetic fixed-key/floor H616 harness with exact rollback and no
production secret or retained state.
