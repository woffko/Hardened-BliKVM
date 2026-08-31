# Exact diagnostic peer identity policy checkpoint

On 2026-08-31, source commit
`fcfde44543b64f1a93eec05ea47c9f92ea6efe7b` completed the first pure Rust
diagnostic peer-identity policy. The `bli-kvm-diagnostic-peer-policy` crate is
`no_std`, forbids unsafe code, depends only on the verified collector-policy
type boundary, and is not linked into any server, broker, process, package,
API, UI, or device runtime.

The packaging audit found five distinct dynamically allocated service UIDs:
server, platform broker, video proxy, control broker, and update supervisor.
Every unit sets the same primary runtime group `hardened-blikvm`. Group access
can restrict admission to a future Unix socket, but it cannot distinguish the
five roles. The producer protocol therefore has no source field.

The pure assignment requires six nonzero, pairwise-distinct UIDs: one future
collector plus the five producers. It also requires one nonzero shared primary
GID. Every zero UID, every one of the 15 duplicate-UID pairs, and a zero shared
GID are rejected before any peer can be mapped.

Connection facts contain only kernel-style optional PID, UID, and GID. Mapping
requires a present PID in `1..=2147483647`, rejects root, rejects the collector
itself, and accepts only one exact producer UID. After the UID selects a role,
the peer GID must equal the configured shared GID as a secondary confinement
check. A matching group with an unknown UID never selects a role. Missing,
zero, oversized, root, collector, unknown, or wrong-group peers fail closed.

The successful result is only `DiagnosticSource`; PID, UID, and GID are not
returned and cannot enter diagnostic records or exported bytes. PID changes do
not change a connection-bound role. PID reuse is not treated as identity: a
future adapter must obtain fresh `SO_PEERCRED` from each accepted stream and
discard the numeric facts after mapping.

Eight focused Rust tests cover:

- all five exact UID-to-source mappings;
- every configured zero UID and a zero shared GID;
- all 15 duplicate-UID pairs;
- missing, zero, maximum-valid, and oversized peer PIDs;
- distinct root, collector, and unknown-UID failures;
- exact shared-GID enforcement without group-based role selection;
- stable role mapping across valid PID changes;
- deterministic mapping that returns only the semantic source type.

The full host suite passed with 121 Python tests plus workspace
format/check/test/Clippy with warnings denied, source, packaging and systemd
boundaries, BasedPyright, production TypeScript build, ESLint, and Prettier.
Two attempts to open the new Rust file through the project LSP hit its
100-document limit, so LSP diagnostics are not claimed; compiler and Clippy
checks passed.

Two clean independent AArch64 workspace/bundle builds produced byte-identical
4,779,137-byte prototype archives:

`db2665ee6134f19b965846c74630c3a210570085c88864242e9ba8b171b90ebb`

The unshipped peer-policy rlibs were byte-identical at 51,266 bytes:

`ed5313eff1ea4c276855aa0c5d38b5497d750fe0c756e67371accd2ff9b1cb5f`

Each peer rlib contained one 48,680-byte AArch64 `lib.rmeta` member,
byte-identical with SHA-256:

`06f080e18278c44c5c8fb1e71bd6dac83b7d2858b2630eb9031b1fe4cc719783`

The single extracted 2,220-byte peer ThinLTO LLVM-bitcode member was
byte-identical with SHA-256:

`d7816361505ac36c118d20de9aeb8559637af6b49bc88c22936bd0a13cc3b04e`

Collector-policy and export rlib, metadata, and ThinLTO pairs also remained
byte-identical with their previously recorded hashes. Direct byte comparisons
passed for both archives and every layer. The archive manifest identifies the
exact source commit and retains `releaseAccepted=false` and
`deviceAccepted=false`. No diagnostic crate is present in the prototype.

No live account lookup, socket, process, or device test was run because the
policy is pure and unlinked. No UID, GID, PID, identifier, live log, configured
path, process output, or device state was captured. A future adapter must safely
resolve the six exact accounts and shared group, use a safe Unix-stream
`peer_cred` API, bind the mapped source to one window session, enforce one active
peer per role, and discard numeric credentials. User-database failure, UID
change, collector restart, concurrency, timeout, systemd confinement, privacy,
browser, and H616 gates remain mandatory.
