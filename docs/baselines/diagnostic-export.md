# Typed diagnostic export checkpoint

On 2026-08-31, source commit
`071013a3d70e48a31ce298d25ac13c871fca04a4` completed the first pure Rust
diagnostic-export slice. The `bli-kvm-diagnostic-export` crate is `no_std`,
forbids unsafe code, has no dependencies, and is not linked into any server,
broker, package, collector, API, UI, or device runtime.

The source boundary is pinned to `blikvm/blikvm` revision
`3e34c04a1dfa84636050585d12d5e585098258ea` and web-server submodule
`932a47740020268ba89c254809d175a371944681`. Its `POST /api/logs` route takes
the log filename from mutable configuration, derives rotated names, selects a
file by modification time, interpolates the selected path into an unquoted
`tail -n 20` shell command, and returns raw output. That route is rejected.

Hardened-BliKVM does not attempt to make arbitrary vendor log text safe with a
regular-expression redactor. The pure export model accepts only fixed semantic
component, event, reason, result, resource, pressure, journal-health, and
severity enums. There is no API field for a message, filename, path, hostname,
address, endpoint, username, request data, arbitrary key, token, cookie, or
authorization value. The crate performs no filesystem, process, clock, network,
or device I/O.

Each record contains only a caller-supplied relative millisecond offset, one
component, and one typed event. Offsets are bounded to a 24-hour window and
must be monotonic. Retry attempts are nonzero and at most 16. One fresh export
holds at most 32 records and emits at most 8,192 bytes. Capacity, time rollback,
invalid events, and output overflow are hard errors; no record or byte is
silently truncated. Output is deterministic canonical ASCII, severity is
derived from the event, and unused fixed-buffer capacity is never exposed.

Seven focused Rust tests cover:

- exact canonical output containing only typed fields;
- invalid time, retry, and clock-rollback rejection without state mutation;
- hard record capacity without silent pruning;
- every component and every nested event enum variant within one export;
- absence of identity, request, credential, and path field names;
- maximum valid time and longest current records at full capacity;
- byte-identical output, private zeroed capacity, and fresh-window restart.

The full host suite passed with 117 Python tests plus workspace
format/check/test/Clippy with warnings denied, source, packaging and systemd
boundaries, BasedPyright, production TypeScript build, ESLint, and Prettier.
Two attempts to open the new Rust file through the project LSP hit its
100-document limit, so LSP diagnostics are not claimed; compiler and Clippy
checks passed.

Two clean independent AArch64 workspace/bundle builds produced byte-identical
4,777,778-byte prototype archives:

`c3e5d05ab70365020734362e1eebe88eb6460f0a631b2076da65900706165ad7`

The unshipped diagnostic-export rlibs were byte-identical at 116,044 bytes:

`e50f611aa5e4a172560ca992875307a42a35dbb2a3eec0fc3f8334646453cfcb`

Each rlib contained one 97,904-byte AArch64 `lib.rmeta` member, byte-identical
with SHA-256:

`25ebd4ba76422228251920f3b86cdcfb9eeed59c32fae4589794afbca4aa2f45`

The single extracted 17,624-byte AArch64 ThinLTO LLVM-bitcode member was
byte-identical with SHA-256:

`509b3ac34afcedb153c1b84c210ba99a8800b748bd72e39ad07f92ff035db718`

Direct byte comparisons passed for both archives, rlibs, metadata members, and
codegen members. The archive manifest identifies the exact committed source
and retains `releaseAccepted=false` and `deviceAccepted=false`. The diagnostic
crate is absent from the prototype archive.

Follow-up source commit `45ed8848f7f2d3cf0ccc2af884478a785f86b7e7`
adds one consumer: the also-unlinked, pure
`bli-kvm-diagnostic-collector-policy`. No process, package, API, UI, or device
runtime links either crate; see `diagnostic-collector-policy.md`.

Final protocol source `64b256d7bddb6b2c1b609dcd11866036dacdb118`
adds a second pure, unlinked consumer with fixed event and mandatory response
frames. No runtime links any diagnostic crate; see `diagnostic-protocol.md`.

No live log, configured log path, rotated file, process output, identifier, or
device state was read for this slice. A future adapter must map one reviewed,
fixed local source into these semantic events without forwarding raw content;
it then needs privilege separation, bounded collection, authenticated export,
privacy review, runtime packaging, and browser tests. Until those gates pass,
the vendor raw-log route and every Hardened diagnostic-export capability remain
unavailable.
