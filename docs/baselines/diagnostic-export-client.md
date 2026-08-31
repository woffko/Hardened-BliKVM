# Bounded diagnostic export client checkpoint

On 2026-08-31, source commit
`3f1984d40753571d0717aa5be6c66067816eaede` completed the first fixed-path Rust
sealed-window export client. The `bli-kvm-diagnostic-export-client` crate is not
linked into the platform broker, server, collector, another process, prototype
archive, package, systemd unit, API, UI, or device runtime.

The only public constructor uses
`/run/hardened-blikvm-diagnostics/export.sock`. There is no public socket path,
timeout, environment, identity, query, filter, logging, persistence, or retry
input. Every operation creates a fresh Unix connection, writes one exact
16-byte request within two seconds, reads the complete response and connection
EOF within five seconds, then drops the stream. Connects are independently
bounded to two seconds. Requiring EOF accepts no trailing frame or connection
reuse; the fixed response buffer rejects byte 8,217.

A sealed read copies at most 8,192 bytes from a canonical sealed response into
a private fixed-capacity owned snapshot. The authenticated listener can produce
that response only from `DiagnosticBundle`. The snapshot retains only the
nonzero collector instance ID, nonzero window ID, omitted-lifecycle count,
record count, exact length, and bundle bytes. Snapshot `Debug` output omits the payload. Zero bindings
cannot be constructed through the safe public binding type, and the client
retains no I/O error, path, peer credential, frame, or arbitrary message in an
error.

Read-sealed is non-mutating. Connect failures are reported as definitely not
sent; write, response, EOF, oversize, read, and canonical-decode failures are
reported as retryable only in the sense that a caller may later issue a new
explicit read. The client itself performs no retry.

Acknowledgement uses the exact snapshot instance/window binding. Applied and
mismatch responses must echo that binding; another binding or response class
is invalid. Once the acknowledgement write begins, write/response failure,
timeout, EOF, oversize, malformed content, wrong operation, or wrong binding is
delivery ambiguous. Cancelling a polled acknowledgement may also occur after
the collector applied it. The required recovery is a later explicit read;
blindly repeating the acknowledgement is forbidden and no retry exists in the
client.

Thirteen focused tests cover:

- fresh connections and no hidden reconnect for reads and acknowledgements;
- no-sealed, sealed, collector-unavailable, acknowledged, and mismatch results;
- exact echoed binding and wrong-operation rejection;
- an owned maximum 8,192-byte payload and unconstructable zero bindings;
- connect failure and bounded pending connect;
- write timeout and write failure;
- whole-response timeout, empty EOF, non-EOF read failure, and byte 8,217;
- malformed and partial frames with read-versus-ack phase classification;
- read and acknowledgement cancellation dropping their one connection; and
- fixed public defaults plus payload-redacted `Debug`.

The complete host job `2906eede98bc4d39b91fdb0b5b3a1aa3` exited zero.
It passed locked workspace format, check, test, and Clippy with warnings denied;
source, packaging, and systemd boundaries; 139 Python tests, compilation, and
BasedPyright; production TypeScript/Vite build; ESLint; and Prettier. Focused
rustdoc with warnings denied also passed. Two LSP attempts hit the project
server's 100-document limit, so LSP diagnostics are not claimed; compiler,
Clippy, tests, and rustdoc are the semantic evidence.

Locked metadata contains 159 packages: 51 workspace and 108 external, with no
missing external license expression. The client adds no external package.
`cargo audit --deny warnings` loaded 1,233 advisories and accepted the exact
graph.

## Independent AArch64 builds

Jobs `25ef6910562646eab7f19fa350a57bae` and
`572becf324ea40c18fd8946739c156e8` used separate target and bundle directories
under `/mnt/d/debug/blikvm/`. Both built the complete locked workspace for
`aarch64-unknown-linux-musl`, passed all shipped ELF checks, built the production
web tree, and produced byte-identical 4,797,360-byte prototype archives:

`6ddcd29c4f270d9f0a988c5b31d4e18efd23a8d1cffa40ce86b59012f437160d`

The client and all diagnostic collector/export artifacts are absent from the
archive. Its manifest identifies exact source `3f1984d`, records a committed
development checkpoint, and retains `releaseAccepted=false` and
`deviceAccepted=false`.

The 135,822-byte client rlibs were byte-identical:

`7cc6d202f32821d84d59fda79a256a50364cb95595211b87501344a442e802b4`

The standalone 117,282-byte metadata files matched:

`7b208af4f485d18b26d5b44099c6e847ef9ee7838c7b9e0fc8c41327f5a40bdb`

Each rlib also contained a byte-identical 117,848-byte `lib.rmeta` member:

`aad299e75475913c6418cb577be122741983a827dbc8899ccdc43f998875041d`

The single 16,700-byte ThinLTO LLVM-bitcode member matched:

`9b027f905d0d9e220dc24bf72e80594a80c228dd58a3041b5c74a57b26e5884e`

The lower 54,160-byte export-protocol rlibs remained identical at
`f4b4f64afa7961d8a03bd75987b816aff510c1a2ca0d6010c97f23999c73c237`;
their 7,636-byte ThinLTO members matched at
`5f66e506e6354db5f01f71e2dc249a4057e9c1d61433b42d095103d4ebe0b483`.
The 116,044-byte typed-export rlibs matched at
`e50f611aa5e4a172560ca992875307a42a35dbb2a3eec0fc3f8334646453cfcb`,
and their 17,624-byte ThinLTO members matched at
`509b3ac34afcedb153c1b84c210ba99a8800b748bd72e39ad07f92ff035db718`.
Direct `cmp` checks passed for both archives and the client rlib and standalone
metadata pairs.

No device test was run for this source checkpoint because no real process links
the client. The preceding live H616 gate used a generated exact-protocol client
and remains valid evidence for the collector/listener, not for this Rust crate.
The next gate must link the client only into the unshipped platform broker,
preserve acknowledgement ambiguity without persistence or HTTP/UI, and prove
the exact native route under reversible H616 staging before a persistence layer
is designed.

Final source `3461d77` subsequently added that integration behind a default-off
Platform Broker feature. It does not change the standalone client's transport
contract. The feature uses reads only, has a separate local schema v2 and exact
Server peer authorization, and passed reversible H616 evidence in
`device-diagnostic-broker.md`. Acknowledgement remains unavailable until
durable persistence exists.
