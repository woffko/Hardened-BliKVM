# Fixed-source diagnostic collector policy checkpoint

On 2026-08-31, source commit
`45ed8848f7f2d3cf0ccc2af884478a785f86b7e7` completed the first pure Rust
fixed-source collector-policy slice. The
`bli-kvm-diagnostic-collector-policy` crate is `no_std`, forbids unsafe code,
depends only on `bli-kvm-diagnostic-export`, and is not linked into any server,
broker, process, package, API, UI, or device runtime.

The source audit selected exactly five process roles that exist in the current
Hardened development runtime: server, platform broker, video proxy, deny-only
control broker, and update supervisor. System-action store, network policy, and
recovery verifier are deliberately not accepted collector sources because no
shipped runtime owns those roles. There is no arbitrary source name. A future
adapter must bind an authenticated local peer to one fixed source before it can
construct a record; source identity is never parsed from text or accepted from
a network request.

Each source starts dormant. Its first record must be `Started` with a nonzero
epoch and sequence zero. Every active record must use the same epoch and exact
next sequence. `Stopped` closes that epoch; restart requires a strictly greater
epoch and a new sequence-zero `Started`. Replay, gaps, epoch mismatch, duplicate
start, post-stop events, and restart without `Started` fail without changing
collector state.

The adapter supplies one collector-owned relative offset. The underlying
exporter enforces monotonic ordering across all sources and the same 24-hour,
32-record, and 8,192-byte bounds. Source epoch and sequence are lifecycle guards
only and never enter exported bytes. If the export is full or time moves
backward, neither the source sequence nor any other collector state advances.
There is no drop-oldest, retry, truncation, or implicit new window.

The event matrix is closed and role-specific:

- lifecycle events are accepted for all five sources;
- rejection reasons are separately allowlisted per role;
- server may report memory, storage, or session pressure;
- platform broker may report only temperature pressure;
- video and control brokers may report only session pressure;
- update supervisor may report only storage pressure;
- operation completion is limited to control broker and update supervisor;
- retry and journal health are limited to update supervisor.

Every other source/event/resource combination is rejected. The policy accepts
no message, bytes, path, unit, hostname, address, endpoint, username, request
data, arbitrary key, identifier, token, cookie, authorization value, shell
fragment, filesystem, process, clock, network, or device interface.

Nine focused Rust tests cover:

- exact mapping of the five fixed sources to exported components;
- lifecycle, stop, and strictly advancing restart epochs;
- pre-start, replay, gap, epoch mismatch, and duplicate-start rejection;
- representative allow/deny cases without state mutation;
- the complete rejection/resource/operation/retry/journal allowlist matrix;
- invalid epoch, time, retry, and global clock rollback handling;
- exact full-capacity behavior without sequence advancement;
- deterministic output without source epoch or sequence fields;
- fresh-window state with no cross-window source lifecycle.

The full host suite passed with 118 Python tests plus workspace
format/check/test/Clippy with warnings denied, source, packaging and systemd
boundaries, BasedPyright, production TypeScript build, ESLint, and Prettier.
Two attempts to open the new Rust file through the project LSP hit its
100-document limit, so LSP diagnostics are not claimed; compiler and Clippy
checks passed.

Two clean independent AArch64 workspace/bundle builds produced byte-identical
4,777,974-byte prototype archives:

`afa109abe09912906274927ed10228d094560f3508048d74790b3015195141a3`

The unshipped collector-policy rlibs were byte-identical at 57,232 bytes:

`befda30e445f6e641e1d3f5f4f261dba3db31ec5d4f0dfde60f7864319f548f8`

Each collector rlib contained one 51,224-byte AArch64 `lib.rmeta` member,
byte-identical with SHA-256:

`18e4debe386b661a64b69d60066c411105aed3767943df655e60ac98c310407d`

The single extracted 5,452-byte collector ThinLTO LLVM-bitcode member was
byte-identical with SHA-256:

`1fda665a36f6a221eeed1fe19037d4101686d6e5cafc9ccbebdca823e8db319f`

The lower diagnostic-export rlib, `lib.rmeta`, and ThinLTO pairs also remained
byte-identical at 116,044, 97,904, and 17,624 bytes with their previously
recorded hashes. Direct byte comparisons passed for both archives and every
collector/export layer. The archive manifest identifies the exact committed
source and retains `releaseAccepted=false` and `deviceAccepted=false`. Neither
pure diagnostic crate is present in the prototype archive.

No device gate was run because this policy has no I/O and was unlinked at this
checkpoint. No live log, configured path, process output, identifier, or device
state was read. The later unlinked runtime core supplies credential-bound
peer-to-role, exact local protocol, disconnect lifecycle, and bounded
backpressure composition. Its transient listener now supplies real monotonic
time, caller-bound accept, bounded I/O, and graceful task cleanup; see
`diagnostic-listener-runtime.md`. A process still needs socket ownership,
crash handling, confinement, authenticated export, privacy review,
packaging, and browser tests. Until then, no diagnostic capability is
advertised.

Final protocol source `64b256d7bddb6b2c1b609dcd11866036dacdb118`
adds a separate pure sibling codec for producer events and mandatory
accepted/rejected responses. It does not link or weaken this policy; a future
runtime must compose both explicitly. See `diagnostic-protocol.md`.

Window-policy source `a00e1a8eb5942d83cd5590b39fbcf9c5cf055fbf`
now composes this collector with lifecycle reservation, sealed handoff and
visible incomplete EOF metadata. It remains pure and unlinked; see
`diagnostic-window-policy.md`.

Peer-policy source `fcfde44543b64f1a93eec05ea47c9f92ea6efe7b`
adds the separate pure UID/GID/PID-to-source mapper without changing this event
matrix. It remains unlinked; see `diagnostic-peer-policy.md`.

Runtime-core source `6de71c28df1eb521108a955d046e098feb3bacb3` now composes the
collector with authenticated peer tokens, exact frames, and window sessions;
see `diagnostic-runtime-core.md`. No shipped process links the composition.

Collector source `632c081afcaacdbd7c491e29cdd7bd5ab339c22a` and final gate
source `28d189a04a7fab3e2b11c1e719b01be0188846cb` now pass the
unshipped socket/process/service and reversible H616 boundaries; see
`device-diagnostic-collector.md`. Real producer integration and authenticated
export remain absent.
