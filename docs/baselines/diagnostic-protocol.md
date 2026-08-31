# Fixed diagnostic producer protocol checkpoint

Source commit `009a9ce87596fd91a584d2cc824f6bd9651b00f1` introduced the first
event-only codec. Review then found that a one-way event frame could not prove
acceptance under collector backpressure. That version and its initial AArch64
pair are intermediate evidence only. Follow-up source commit
`64b256d7bddb6b2c1b609dcd11866036dacdb118` adds a mandatory typed response;
only the final commit and replacement builds below are accepted.

The `bli-kvm-diagnostic-protocol` crate is `no_std`, forbids unsafe code,
depends only on `bli-kvm-diagnostic-export`, and is not linked into any server,
broker, process, package, API, UI, or device runtime. Its request is one exact
eight-byte frame containing schema, closed event tag, two typed arguments, and
four zero reserved bytes. It has no source, UID/PID, epoch, sequence, time,
path, message, request payload, identifier, or arbitrary byte field.

`Started` and `Stopped` cannot be encoded by a producer. The future collector
must derive source from kernel peer credentials and assign lifecycle, epoch,
sequence, and relative monotonic time. The current five accounts have distinct
dynamically allocated UIDs but use one shared runtime group, so group membership
is not accepted as role identity. Exact UID-to-role resolution and one active
peer per role remain runtime requirements.

After every valid event, the producer must wait for one exact four-byte
response. The collector can report accepted or typed rejection: event not
allowed, backpressure, or collector unavailable. Acceptance is permitted only
after policy state advances. Invalid request frames are connection-fatal and
receive no response. No pipelining or implicit acceptance exists.

Eleven focused Rust tests cover:

- every producer-owned semantic event and nested enum variant;
- exact request frames without identity, lifecycle, sequence, or time;
- producer rejection of collector-owned lifecycle events;
- every request length from zero through 33, every schema byte, and every
  nonzero reserved-byte value;
- every unknown request tag;
- all 458,752 request tag/argument-byte combinations;
- invalid retry and nested argument domains;
- deterministic decode/re-encode;
- exact accepted and three typed-rejection response frames;
- all 65,536 response tag/argument combinations;
- response lengths, every schema byte, and every nonzero reserved value.

The full host suite passed with 119 Python tests plus workspace
format/check/test/Clippy with warnings denied, source, packaging and systemd
boundaries, BasedPyright, production TypeScript build, ESLint, and Prettier.
Two attempts to open the new Rust file through the project LSP hit its
100-document limit, so LSP diagnostics are not claimed; compiler and Clippy
checks passed.

Two clean independent AArch64 workspace/bundle builds from the final source
produced byte-identical 4,778,341-byte prototype archives:

`96b28ecb226fc312aaaa7e0d1dfa0d5f6c50693b7380dbcc44e544910684211c`

The unshipped protocol rlibs were byte-identical at 47,260 bytes:

`f0ad8be462ae6b51546e3170c4679c7800232b3c8b8d52808448d3eed0814e9f`

Each protocol rlib contained one 43,152-byte AArch64 `lib.rmeta` member,
byte-identical with SHA-256:

`fbdc5c81e13a5402366ecc5832e368709c25f1cf64fa1b6b4627de1489c36466`

The single extracted 3,680-byte protocol ThinLTO LLVM-bitcode member was
byte-identical with SHA-256:

`6f89193c0e66af308e5d666c7200b96e333cb3724e57c4d553991ac166bda0cb`

The lower collector-policy and diagnostic-export rlib, metadata, and ThinLTO
pairs also remained byte-identical with their previously recorded hashes.
Direct byte comparisons passed for both archives and every protocol/collector/
export layer. The archive manifest identifies the exact final source and
retains `releaseAccepted=false` and `deviceAccepted=false`. No diagnostic crate
is present in the prototype archive.

No device test was run because the codec is pure and unlinked. No live log,
configured path, process output, identifier, or device state was read. There is
still no Unix socket, peer-credential adapter, collector process, window state,
clock owner, authenticated API/UI, package, or advertised diagnostic
capability. Those remain separate gates described in
`api/diagnostic-producer-v1.md`.

Window-policy source `a00e1a8eb5942d83cd5590b39fbcf9c5cf055fbf`
now consumes the response types for accepted, policy-rejected, backpressured,
and unavailable outcomes. It remains pure and unlinked; see
`diagnostic-window-policy.md`.

Runtime-core source `6de71c28df1eb521108a955d046e098feb3bacb3` now performs the
first exact decode/submit/encode composition. Invalid frames terminate without
a response; canonical policy, backpressure, and unavailable responses preserve
the connection. The core remains outside every shipped process; see
`diagnostic-runtime-core.md`.

Listener source `89050939af218ddbdd7143fb63e320e1ca9eac67` now owns exact
frame reads and response writes with fixed time limits while preserving the
same codec contract; see `diagnostic-listener-runtime.md`.
