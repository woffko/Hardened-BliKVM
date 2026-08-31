# Typed diagnostic producer client checkpoint

On 2026-08-31, source commit
`49173a76d527aef9e41929acf789ac789ab66495` completed the first fixed-path
Rust diagnostic producer client. The
`bli-kvm-diagnostic-producer-client` crate is not linked into any server,
broker, proxy, supervisor, collector process, package, systemd unit, API, UI,
or device runtime.

The only public constructor uses the compile-time socket
`/run/hardened-blikvm-diagnostics/collector.sock`. There is no public path,
timeout, configuration, environment, identity, or retry input. Connection,
full-frame write, and exact-response read have fixed two-, two-, and
five-second limits. One Tokio mutex serializes callers around one persistent
Unix stream so frames and responses cannot interleave.

The client encodes an event before acquiring or opening a stream. Invalid
producer events, connect timeout, and connect failure are therefore returned
as definitely not sent. Immediately before any I/O, the stream is moved out of
the stored option. If the send future is cancelled, the local stream is dropped
and no later call can reuse an uncertain protocol state.

Once frame writing begins, the client is deliberately conservative. Write
timeout/failure, response timeout, EOF, other response-read failure, and
noncanonical response are all returned as delivery ambiguous. The stream is
discarded and no retry occurs. A later caller may make a new explicit `send`,
which establishes a fresh connection for a new deliberate attempt. The client
does not claim whether the ambiguous event advanced collector state.

A canonical accepted response becomes `Accepted`. Event-not-allowed,
backpressure, and collector-unavailable responses remain distinct typed
rejections and keep the protocol connection usable. Errors contain only a
coarse not-sent reason or ambiguous stage; they retain no I/O error, path,
frame bytes, response bytes, identity, or arbitrary text. The client itself
implements neither `Clone` nor `Debug`.

Eleven focused tests cover:

- two exact accepted events on one reused connection;
- every typed rejection without reconnect;
- invalid event and missing socket as definitely not sent;
- bounded pending connect;
- write timeout and write failure ambiguity;
- response timeout, EOF, and non-EOF read ambiguity;
- malformed response followed by one explicit reconnecting send;
- cancellation dropping an in-flight stream before the next explicit send;
- concurrent callers serialized into complete frames; and
- exact fixed defaults and producer-owned event encoding.

The full host suite passed with 134 Python tests plus locked workspace format,
check, test, and Clippy with warnings denied; source/packaging/systemd
boundaries; Python compilation and BasedPyright; production TypeScript build;
ESLint; and Prettier. Two attempts to obtain LSP diagnostics for the new file
hit the project server's 100-document limit, so LSP diagnostics are not
claimed; compiler and Clippy checks passed.

The workspace now contains 156 locked packages: 48 workspace and 108 external,
with no missing external license expression. This client adds no external
package. `cargo audit --deny warnings` loaded 1,233 advisories and accepted the
exact lock graph.

## Independent AArch64 builds

Two clean independent workspace/bundle builds produced byte-identical
4,780,599-byte prototype archives:

`cc14e90d4a6afb4b8e9f9ce1da77c85243a09c52875d241005c59428be5e5a90`

The archives contain no producer-client artifact. Their manifest identifies
the exact source commit and retains `releaseAccepted=false` and
`deviceAccepted=false`; all six shipped binaries and the web tree matched.

The unshipped producer-client rlibs were byte-identical at 74,826 bytes:

`e45f4751e3ef51c5165b7f71f2163dd86d2260a4b005b57902b3acd231a71800`

Each contained one 67,088-byte `lib.rmeta` member with SHA-256:

`5c15a5f2c3af7ddf2d5959a83dbffa2817b855a71ba083091594cf5189ed878d`

The single 7,004-byte ThinLTO LLVM-bitcode member matched with SHA-256:

`5ad889de70d6ae80da614be55c6f960f22994dcb7e1e084f2b4730a0127b46d4`

The protocol and export rlib, metadata, and ThinLTO pairs also remained
byte-identical with their previous hashes. Direct comparisons passed for every
member used by this client.

No device test was run because no real process links the client. No device
state, live log, process output, account, socket, path contents, or numeric
identity was read. The existing collector H616 acceptance remains valid but
used a fixed generated test client, not this Rust crate.

The next gate must select one non-critical producer, define which semantic
events it may emit and how ambiguous delivery affects its own operation, then
prove reversible integration without making diagnostics a dependency for
service startup or hardware behavior.
