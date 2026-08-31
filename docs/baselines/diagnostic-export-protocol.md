# Instance-bound diagnostic export protocol checkpoint

On 2026-08-31, source commit
`6189f5bc8a5ebb2edfac895f885db7cb940f6229` completed the first pure
sealed-window export protocol. The `bli-kvm-diagnostic-export-protocol` crate
is unlinked from every process, socket, service, client, package, API, UI, and
device runtime.

Every read or acknowledgement request is exactly 16 bytes. Reading requires a
zero binding; acknowledgement requires both a nonzero OS-random collector
instance ID and nonzero window ID. The window policy and every runtime layer
now expose and validate that pair, so a delayed acknowledgement for window `1`
from an earlier collector process cannot clear window `1` after restart.

Every response has one canonical 24-byte header and at most 8,192 payload
bytes. Fixed tags represent no sealed window, a sealed snapshot,
acknowledgement success, acknowledgement mismatch, or collector unavailable.
The public sealed encoder accepts only `&DiagnosticBundle`; it has no
arbitrary-byte encoder. The decoder validates exact total length, schema, tag,
binding, omission/count bounds, payload presence, and six mandatory-zero
reserved bytes. Neither response type nor its fixed buffer implements `Debug`.

The codec is `no_std`, forbids unsafe code, allocates nothing, and contains no
path, identity, clock, arbitrary query, raw-log text, retry, persistence,
filesystem, process, socket, or device surface. Invalid request frames are
intended to be connection-fatal in the future adapter.

Five focused codec tests cover exact read/ack bytes, invalid zero bindings,
typed-bundle-only sealed encoding, every fixed response, borrowed payload,
unknown tags, reserved fields, count/length mismatch, truncated frames, and
empty sealed-bundle rejection. The existing 11 window-policy and 24 runtime/
listener tests pass with the new instance-bound acknowledgement. Two LSP
diagnostic attempts hit the project server's 100-document limit, so LSP
diagnostics are not claimed; compiler and Clippy checks with warnings denied
passed.

The full host suite passed with 137 Python tests plus locked workspace format,
check, test, and Clippy; source, packaging, and systemd boundaries; Python
compilation and BasedPyright; production TypeScript build; ESLint; and
Prettier. Locked metadata contains 157 packages: 49 workspace and 108 external,
with no missing external license expression. `cargo audit --deny warnings`
loaded 1,233 advisories and accepted the exact graph.

## Independent AArch64 builds

Two clean independent workspace/bundle builds produced byte-identical
4,796,389-byte prototype archives:

`1bf4c0f66d9f460bb5c431812240a2f34efe91534aa5e728e6787c573dd27027`

The new protocol is absent from the archive. Its manifest identifies source
`6189f5b` and retains `releaseAccepted=false` and `deviceAccepted=false`.

The 54,160-byte export-protocol rlibs were byte-identical:

`f4b4f64afa7961d8a03bd75987b816aff510c1a2ca0d6010c97f23999c73c237`

Their 45,808-byte `lib.rmeta` and 7,636-byte ThinLTO members matched with
SHA-256, respectively:

`0670d86e0855daa42753292c1e1b40e34f177672b478dfca4d80698ad8da2a4c`

`5f66e506e6354db5f01f71e2dc249a4057e9c1d61433b42d095103d4ebe0b483`

The updated 101,916-byte window-policy rlibs matched:

`ec13226a3e276554767432ea695dc71bb283fb2cff9a694cf63233defabdbeb8`

Their 87,360-byte metadata and 13,600-byte ThinLTO members matched:

`fe1fe2adf9b8bf1fcf93b521a873df223d6833153870a6a071775692c4cba40d`

`dd6bb55dec286df14c36e00eae375084419defe1dba34e0b7daa4311b50cf1cb`

The updated 287,736-byte peer-adapter rlibs matched:

`6af1668b6fc2f865d68ba3692d9ca24d475a303903d5898ce0fe0d1230c51aa0`

Their 260,560-byte metadata and 25,308-byte ThinLTO members matched:

`c915756515ea77a883537d3b098b59cdb4aa38625b5fba9d622882399c2e1873`

`a34e7d52ae3b4f5fbc5f485116e722d7ffb841164401b7e952c1ecb9d75a99da`

The unchanged lower export rlib also remained byte-identical. Direct byte
comparisons passed for both archives and all four protocol/window/adapter/
export rlib pairs.

No device test was run because no socket or runtime links the codec. The next
checkpoint must authenticate one exact non-root consumer by kernel peer
credentials, allow one bounded request/response per connection, keep producer
ingestion independent of stalled export I/O, and preserve the sealed snapshot
until an exact instance/window acknowledgement succeeds. Persistence and HTTP/
UI remain later, separate gates.
