# Transient diagnostic listener runtime checkpoint

On 2026-08-31, source commit
`89050939af218ddbdd7143fb63e320e1ca9eac67` completed the first transient
Unix listener and asynchronous I/O owner for the authenticated diagnostic
runtime. The implementation remains in the
`bli-kvm-diagnostic-peer-adapter` library and is not linked into any server,
broker, executable, package, systemd unit, API, UI, or device runtime.

`DiagnosticCollectorRuntime::from_current_process` resolves only the fixed six
accounts and shared group, verifies that the process eUID is the collector
UID, obtains a fresh nonzero instance binding from operating-system entropy,
and anchors a real monotonic `Instant`. Entropy reads have a bounded retry
count and no timestamp, PID, address, path, or numeric credential is used as a
fallback instance identity.

The runtime accepts a caller-owned, already-bound Tokio `UnixListener`. It does
not accept a path, bind or unlink a socket, change permissions, read an
environment variable, or own service configuration. Every accepted stream is
mapped through connection-bound `peer_cred` before a frame can be read. An
unknown, root, collector, wrong-group, missing-PID, or duplicate-role peer is
closed without creating a diagnostic session or receiving a response.

One mutex serializes every core mutation. Monotonic time is sampled only after
that mutex is acquired, so two concurrent producers cannot observe lock order
as clock rollback. A fixed one-second tick drives time-based window rotation.
The public snapshot and acknowledgement methods expose only the existing
bounded semantic model and exact sealed-window ID.

Long-running producers may remain idle for their process lifetime. This is a
bounded resource because the peer policy and window manager permit only one
connection for each of five fixed roles. After the first byte arrives, the
remaining seven request bytes must arrive within five seconds. The exact
four-byte response must be written within five seconds. EOF, partial-frame
timeout, read failure, or response failure closes the stream and performs
bounded disconnect accounting. A malformed complete frame receives no
response and is terminated by the runtime core.

Shutdown has priority over accepting or starting another frame. A response for
a frame already committed to policy still receives its one bounded write
attempt, after which the connection records disconnect. Completed tasks are
reaped before new accepts, all remaining tasks receive shutdown, and the
listener waits for their bounded cleanup. A runtime instance can enter its
serve loop only once, preventing reuse of an old instance binding on another
listener.

Ten focused listener tests, in addition to the fourteen adapter/core tests,
cover:

- nonzero OS-random instance binding and nonzero fixed limits;
- an idle long-running peer followed by a partial-frame timeout;
- blocked response timeout with lifecycle disconnect;
- preservation of a committed mandatory response across shutdown;
- malformed-frame termination with no response;
- concurrent sources sharing one monotonic policy order;
- real Unix-listener duplicate-role rejection and graceful cleanup;
- real unknown-UID rejection before `Started`;
- forty real reconnect/EOF cycles with task reaping and exact sealed-window
  acknowledgement; and
- refusal to restart one runtime instance on a second listener.

The full host suite passed with 122 Python tests plus locked workspace format,
check, test, and Clippy with warnings denied; shell/source/systemd boundaries;
Python compilation and BasedPyright; Node syntax; production TypeScript build;
ESLint; and Prettier. Twelve additional direct runs of the 24 Rust tests also
passed. Two attempts to obtain diagnostics for the new listener file hit the
project LSP server's 100-document limit, so LSP diagnostics are not claimed;
compiler and Clippy checks passed.

Locked offline metadata still resolves 152 packages: 46 workspace and 106
external, with no missing external license expression. The production listener
reuses already locked `rand_core 0.6.4` with its `getrandom 0.2.17` path; the
test-only `tempfile` dependency was also already locked. No external package
was added. `cargo audit --deny warnings` refreshed and loaded 1,233 RustSec
advisories and accepted the exact lock graph.

## Independent AArch64 builds

Two clean independent workspace/bundle builds produced byte-identical
4,780,338-byte prototype archives:

`b849a8252e18dd9270dd2a641bcbce92d9ec67ae9951a7946629c036995c8430`

The unshipped listener/adapter rlibs were byte-identical at 287,064 bytes:

`0f1a0949d107f6a415a2bdd2aa24b521d3f269b42f2d0dde30d0f64ab49b1727`

Each rlib contained one 259,920-byte AArch64 `lib.rmeta` member,
byte-identical with SHA-256:

`1bd2ceb68829f4db279edbd24f7bc6cdfd70f2ce0b602e1aab83defeadf5fb12`

The single 25,276-byte listener/adapter ThinLTO LLVM-bitcode member was
byte-identical with SHA-256:

`b30e2caea187863bac2b56dc1295cd53a3610642ee8aeb2e613598a3176c6fe5`

The direct entropy path was also byte-identical:

| AArch64 rlib | Bytes | SHA-256 |
| --- | ---: | --- |
| rand_core 0.6.4 | 141,642 | `2f4f459fe5da83723236274a5ebc576b29e13e138b875a2d694c53a0e406ac29` |
| getrandom 0.2.17 | 83,694 | `30b9ec054652cb7736b45646753749a335717503085623e15169151ed078f25b` |

Their metadata and ThinLTO members matched directly. The other `getrandom`
variant already present in the full workspace target graph also matched. Every
lower diagnostic protocol/window/peer/collector/export rlib, metadata member,
and ThinLTO member remained byte-identical with the runtime-core checkpoint;
the `nix 0.30.1` pair also remained identical.

The archive manifest identifies the exact source commit and retains
`releaseAccepted=false` and `deviceAccepted=false`. All six shipped runtime
binaries and the web tree were identical between builds. No diagnostic crate
is packaged or linked into a shipped process; `cargo tree --invert` ends at the
peer-adapter crate.

No device test was run. Host tests used only temporary Unix sockets and the
current test process credentials; they created no named account and persisted
or printed no numeric peer identity. No live log, configured path, process
output, identifier, or device state was read. There is still no collector
binary/account, socket-path owner, systemd socket/service, producer integration,
persistence, authenticated export API/UI, package, or advertised capability.
The next gate must add those account/socket/process boundaries as unshipped
templates and pass a fully reversible isolated H616 test before any product
integration.

Collector source `632c081afcaacdbd7c491e29cdd7bd5ab339c22a` and final gate
source `28d189a04a7fab3e2b11c1e719b01be0188846cb` later add that
unshipped process/account/systemd boundary and pass the reversible H616 gate;
see `device-diagnostic-collector.md`. The result remains outside the product
bundle and exposes no export API or capability.
