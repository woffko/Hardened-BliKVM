# Pure system-action policy checkpoint

On 2026-08-31, source commit
`6c75377c09d99f46b67ed2be9a34b7909401b08a` completed the first pure Rust
reboot/shutdown policy slice. The `bli-kvm-system-action-policy` crate is
`no_std`, forbids unsafe code, has no dependencies, and is not linked into any
server, broker, package, system service, or privileged runtime.

The model contains only semantic `Reboot` and `Shutdown` actions. It has no
shell or executable paths, process spawning, filesystem, clock, network, or
device access. Its only public eligibility constructor is disabled; the
enabling constructor exists under `cfg(test)` only. Therefore no production
caller can currently obtain a dispatch intent.

Every new request binds a nonzero operation ID, session, confirmation, issuance
time, and origin boot. Preparation requires all of the following supplied facts:

- enabled capability and ready backend;
- verified local administrator, recent reauthentication, and confirmation no
  older than 10 seconds;
- matching boot binding and at least 60 seconds of boot uptime;
- idle update and storage guards;
- ready recovery path and post-boot observer.

The policy rechecks those guards before dispatch. It first emits a
`PersistPrepared` snapshot and only after a separate transition emits a
`PersistDispatching` snapshot plus the semantic intent. A future adapter must
durably commit each snapshot before continuing; this crate does not claim that
persistence happened. A guard failure between the two transitions records a
terminal rejection instead of dispatching.

Restored prepared or dispatching state is never executed automatically. A
prepared record from another boot is rejected without dispatch. Dispatching
state remains indeterminate until an explicitly requested post-boot acceptance
observes a different nonzero boot binding, at least 30 seconds of uptime, idle
update/storage state, ready recovery, and ready health. Terminal results replay
idempotently, conflicting operation IDs fail closed, accepted actions impose a
60-second cooldown, and every boot has its independent 60-second startup guard.

The storage-neutral snapshot is bounded to 16 records and never silently prunes
idempotency history. A full journal rejects new actions. Restore validates the
schema, IDs, bindings, timestamps, phase fields, duplicate operation IDs, and
single-pending invariant. Generation overflow and stale transition tickets fail
without state mutation. There is intentionally no durable codec, atomic or
dual-copy store, compactor, or privileged actuator in this slice.

Twelve focused Rust tests cover:

- public disabled eligibility and zero state mutation;
- local-admin, reauthentication, confirmation, backend, update, storage,
  recovery, and observer failures;
- confirmation age, boot binding, and startup bounds;
- separate prepared and dispatching generations across restore;
- pending-state replay and concurrency rejection;
- prior-boot prepared-state rejection without dispatch;
- changed-boot and post-boot health acceptance;
- guard changes between preparation and dispatch;
- synchronous dispatch rejection and stale intent handling;
- operation-ID conflicts, terminal replay, and cooldown;
- full-journal fail-closed behavior without implicit pruning;
- corrupt snapshot and generation-exhaustion rejection.

The full host suite passed with 89 Python tests plus workspace
format/check/test/clippy, source, packaging and systemd boundaries,
BasedPyright, production TypeScript build, ESLint, and Prettier. Two attempts to
obtain LSP diagnostics for the new file hit the project server's 100-document
limit, so new-file LSP diagnostics are not claimed; `cargo check` and Clippy
with warnings denied both passed.

Two clean independent AArch64 workspace/bundle builds produced byte-identical
4,777,145-byte prototype archives:

`54f2f69f7a0107a6275953050cf58cb15b1c7678792ae4f9fd137aeb61a1bcdb`

The unshipped policy rlibs were byte-identical at 192,722 bytes:

`fcc77d20e03c3585d5ab662e6e326ac53e1481e3244ffd0c37c255c5d68d7474`

Each rlib contained one 160,088-byte AArch64 `lib.rmeta` member, byte-identical
with SHA-256:

`fb27c7a82e285e297c4df37e4ea8a6abaa30a3fbe375946e0e2333e82f65684e`

The single extracted AArch64 ThinLTO LLVM-bitcode member was byte-identical at
31,228 bytes:

`b11227300051d0d965c9cca324058af1d6bb5831281bba08a3b89c4162e2e513`

Direct byte comparisons passed for both archives, both rlibs, both metadata
members, and both codegen members. The policy is absent from the prototype
archive, whose manifest identifies the exact committed source and retains
`releaseAccepted=false` and `deviceAccepted=false`.

No reboot, shutdown, hostname, service, filesystem, log, clock, Power, or Reset
action was executed. Runtime remains unavailable until a reviewed durable
journal, privilege-separated fixed actuator, authenticated API/UI confirmation,
visible progress, update/storage coordination, boot observation, packaging,
reboot/shutdown persistence, failure injection, and explicit device acceptance
all pass. Hostname and log handling remain separate unavailable capabilities.
