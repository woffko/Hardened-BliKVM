# Authenticated diagnostic runtime core checkpoint

On 2026-08-31, source commit
`6de71c28df1eb521108a955d046e098feb3bacb3` completed the first in-memory
composition of the fixed diagnostic peer, producer protocol, and bounded
window policy. The composition remains inside the
`bli-kvm-diagnostic-peer-adapter` crate and is not linked into any server,
broker, process, package, API, UI, systemd unit, or device runtime.

The public production path obtains an `AuthenticatedDiagnosticPeer` only from
Tokio `UnixStream::peer_cred` after fixed-name account resolution and the pure
UID/GID/PID policy accept the peer. The token contains only a semantic source,
is consumed by `DiagnosticRuntimeCore::accept`, and implements neither
`Clone` nor `Debug`. Numeric kernel credentials cannot enter the runtime core.

An accepted peer becomes an opaque `DiagnosticConnection`. It also implements
neither `Clone` nor `Debug`, and its private session is bound to one runtime
instance. A foreign runtime returns `SessionMismatch` without clearing or
consuming the valid owner handle. Normal disconnect clears the handle only
after the window manager accepts the disconnect, so a failed cross-instance
operation cannot strand the original connection.

`handle_frame` decodes exactly one canonical eight-byte producer frame and
returns exactly one canonical four-byte response. Accepted, event-policy,
backpressure, and collector-unavailable results use the response types already
owned by the protocol and window layers. Rejections do not advance policy
state and keep the logical connection available for a later frame.

A malformed frame is connection-fatal and receives no response. The core
immediately runs the same disconnect path used for EOF and returns only a
typed termination reason plus the bounded recorded/incomplete lifecycle
outcome. A clock or other window failure also terminates instead of silently
retrying. The window manager remains authoritative for one live and one sealed
window, exact acknowledgement, rotation, lifecycle reservation, visible
omissions, and hard second-window backpressure.

Fourteen focused adapter/runtime tests cover:

- exact fixed-name resolution and every user/group lookup failure;
- current collector UID and duplicate assignment rejection;
- missing, negative, unknown, and wrong-group peer credentials;
- a real local Tokio stream pair with connection-bound `peer_cred`;
- accepted frame, exact response, recorded EOF, and closed-handle rejection;
- policy rejection with no mutation and a still-usable connection;
- malformed-frame termination with no response and a recorded stop;
- clock-failure termination with visible incomplete lifecycle metadata;
- duplicate authenticated source rejection;
- cross-runtime session rejection without consuming the owner handle;
- exact backpressure from a second full window while the connection remains
  usable; and
- bounded snapshots, rotation, sealed-window access, and exact acknowledgement.

The full host suite passed with 122 Python tests plus locked workspace format,
check, test, and Clippy with warnings denied; shell/source/systemd boundaries;
Python compilation and BasedPyright; Node syntax; production TypeScript build;
ESLint; and Prettier. The same adapter document had already reached the
project LSP server's 100-document limit twice, so LSP diagnostics are not
claimed for this checkpoint; compiler and Clippy checks passed.

## Independent AArch64 builds

Two clean independent workspace/bundle builds produced byte-identical
4,780,015-byte prototype archives:

`766949b06307c82f7e19f23d72eafbecf050c6ebf03a70e79bac655fe8c3a1b6`

The unshipped runtime/peer-adapter rlibs were byte-identical at 88,550 bytes:

`dd30b60c6767106108a3c87c5b08cf7332578c25219bdbe10a1297f4bbc12414`

Each adapter rlib contained one 70,376-byte AArch64 `lib.rmeta` member,
byte-identical with SHA-256:

`b89c34b14ec1dd59f9bf21fae3de92049e70b0d255449d5b09c7f5cc8e2a9d04`

The single 16,752-byte adapter ThinLTO LLVM-bitcode member was byte-identical
with SHA-256:

`4de5050aba3f82722d7a79b4942803d4a956e03f1dbe1c264f9d1dd2e2bd1bf4`

The window-policy rlibs were byte-identical at 100,704 bytes with SHA-256
`0340b7234ce8f91cc5f5d549ce4540bad8a1d8f76872cdd3d5a24deae4476aff`.
Their 86,216-byte metadata matched with SHA-256
`9ba725d2a95ffb97cab640b48c9a106da26fc473b94346973d9ee8eb2f8201a1`;
their 13,532-byte ThinLTO member remained byte-identical with SHA-256
`2958e2a30791f196854c8d129259d08eb690969a023a55a20b058f7a775c854d`.

Direct byte comparisons also passed for every lower layer:

| AArch64 rlib | Bytes | SHA-256 |
| --- | ---: | --- |
| diagnostic protocol | 47,260 | `f0ad8be462ae6b51546e3170c4679c7800232b3c8b8d52808448d3eed0814e9f` |
| diagnostic peer policy | 49,146 | `eb46a988514414a323a5499502c0da418f5f66a7059dcc4dba66ebece8c36b34` |
| diagnostic collector policy | 57,232 | `befda30e445f6e641e1d3f5f4f261dba3db31ec5d4f0dfde60f7864319f548f8` |
| diagnostic export | 116,044 | `e50f611aa5e4a172560ca992875307a42a35dbb2a3eec0fc3f8334646453cfcb` |
| nix 0.30.1 | 776,772 | `4c9fc33304d3e20341a74a0e305c38df36440706131a3e01fbd9272ad446e0dd` |

Their member lists, `lib.rmeta` members, and ThinLTO members were also compared
directly and matched. The archive manifest identifies the exact source commit
and retains `releaseAccepted=false` and `deviceAccepted=false`. All six shipped
runtime binaries and the web tree were identical between builds. No diagnostic
crate is packaged or linked into a shipped process; the workspace dependency
graph still ends at the peer-adapter crate.

No device test was run for this checkpoint. No live log, process output,
configured log path, identifier, account, socket, or device state was read or
created. There is still no listener, async connection handler, bounded I/O
timeout, real monotonic clock, fresh runtime-instance source, collector
account/process, systemd unit, persistence, authenticated export API/UI,
package, or advertised capability. The next gate is a bounded Unix connection
handler and listener with explicit clock and instance ownership, followed by a
reversible isolated account/systemd H616 test before any product integration.
