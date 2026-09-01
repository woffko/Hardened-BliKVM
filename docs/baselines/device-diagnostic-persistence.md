# Synthetic H616 diagnostic persistence checkpoint

On 2026-09-01, source commit
`7bef2bc827c4d660b00a9700e16cef39a58cde86` added the unshipped
zero-argument `bli-kvm-diagnostic-persistence-gate`, its static test-only unit,
and a marker/watchdog/account-CAS rollback orchestrator. The exact test boundary
is in `../design/diagnostic-persistence-gate-v1.md`.

No gate binary, unit, credential, key, store, coordinator, persistence library,
or diagnostic runtime is present in the default prototype payload. The gate
does not enable Platform Broker diagnostics, HTTP, UI, packaging, or a
production service.

## Exact synthetic flow

The gate ran the real fixed systemd key loader, encrypted store, export client,
persistence adapter, coordinator, and journal against a typed in-process export
protocol peer. Its external floor was a fixed process-memory-only test value;
it is not a trusted or persistent anchor.

Starting from encrypted empty generation one, the gate:

- persisted window one pending at generation two;
- advanced the synthetic floor, reread the exact binding, acknowledged it, and
  persisted generation three;
- persisted window two pending at generation four;
- advanced the floor, reread it, confirmed the ACK request had reached a
  deliberately stalled server, then cancelled the runtime future;
- recovered without collector I/O and persisted ambiguous generation five;
- advanced the floor, reread the still-sealed window, and persisted pending
  generation six;
- advanced the floor, reread and acknowledged it once, producing generation
  seven; and
- performed a final no-sealed read and confirmed idle generation seven.

The accepted fixed result is seven reads and three acknowledgement requests, one
cancelled acknowledgement, and zero automatic retries. Final authenticated A/B
selection was clean at generation seven with exactly two acknowledged entries.

## Host and reproducible build evidence

Four focused Rust tests cover the scripted typed sequence, mismatch
nonmutation, fixed receipt/counts, and fixed paths/bindings. Eleven Python tests
cover binary/config validation, POSIX-shell syntax, preflight/run/final facts,
credential secrecy, exact rollback metadata/account CAS, sysusers auxiliary
files, mutation ordering, path markers, and physical-action/broad-delete
exclusions. Thirty-six packaging tests include the unshipped crate/unit and all
lower-layer consumer allowlists.

Full Longrun job `e55709c19a30438fb9f66d047e946a9c` exited zero with the
complete locked Rust/all-features/Clippy, source/systemd verify/security,
170-Python-test, TypeScript, BasedPyright, frontend build/lint, and formatting
matrix. The project LSP server was retried for the gate source and returned
`document limit exceeded: 100/100`; LSP diagnostics are not claimed.

The locked graph contains 175 packages: 58 workspace and 117 external, with no
missing external license expression. The gate adds one workspace package and
no external dependency. `cargo audit` loaded 1,233 RustSec advisories and
reported no vulnerability in the 175 locked dependencies.

Longrun jobs `15af169bc6334165bbfe326140f28a08` and
`52ee0a1dbe2a4baf91ae9e8085fe59ff` used independent target and bundle roots.
The byte-identical 4,803,076-byte default prototype archives have SHA-256:

`e02729175d8a8f76acd3e2592755b74733dba2d74d039174f2f80713512288a3`

Both manifests identify the exact clean source commit and retain
`releaseAccepted=false` and `deviceAccepted=false`. The archive contains no
gate binary/unit or diagnostic persistence artifact. All six shipped executable
hashes match between builds.

The 711,936-byte gate executables are byte-identical AArch64 static PIE:

`f9d5b603ab1499921a356a2be0d9366f3741217e19e87b9005babf108c1d2b22`

The 1,224-byte static unit has SHA-256:

`c8300349aec2b393d81b69d453733784c9cbe704b065f75460229ab63e4a15f1`

## Reversible H616 evidence

Longrun job `9b7745572aaa41c9b9fbc4177ada3f99` exited zero on the designated
test-only H616. The private mode-0600 receipt is
`evidence/private/device-diagnostic-persistence.json`; it contains only
allowlisted semantic facts and public artifact hashes.

Before mutation, the gate independently confirmed the production diagnostic
root, runtime root, credential, target binary, runtime unit, temporary
identities, marker stage, and watchdog were absent; KVMD and KVMD Nginx were
active and rootfs was read-write.

Before the first account or persistent-path write, the orchestrator predicted
the exact sysusers transition, retained all four account databases in a private
marker-owned stage, stored before/expected hashes, wrote the exact rollback
script, and armed a ten-minute systemd watchdog.

The live result proved:

- the exact binary hash and systemd credential runtime layout were accepted;
- both encrypted copies were equal, mode 0600, and final generation was seven;
- the journal contained exactly two acknowledged entries;
- request totals were exactly seven reads and three acknowledgement requests;
- one acknowledgement was cancelled only after the server signalled receipt
  and entered the deliberate response stall;
- ambiguity recovery performed no collector read and no automatic retry;
- the static sandbox source and runtime properties passed; and
- account databases were restored exactly.

The separate final-state SSH request confirmed the production/runtime roots,
credential, binary, unit, temporary identities, stage, and watchdog were
absent; KVMD/KVMD Nginx remained active and rootfs remained read-write. No
physical Power/Reset, reboot, shutdown, KVMD restart, hardware action, clock
write, network mutation, or raw vendor log was invoked.

An initial job exited locally with code two because the endpoint validator
requires a credential-free `ssh://` URL rather than a bare address. Asset
validation completed, but routing failed before password read, SSH, receipt,
or device staging. The corrected job above is the only device attempt.

## Remaining boundary

This accepts ordinary H616 systemd credential delivery, encrypted-store Unix
I/O, fixed local protocol transport, and synthetic cancellation/recovery. It
does not accept a production key/source ceremony, trusted external floor,
power-cut or reboot durability, shipped daemon/unit, default broker linkage,
authenticated diagnostic HTTP API, Diagnostics tab, packaging, privacy/soak,
or release.
