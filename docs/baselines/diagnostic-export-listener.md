# Authenticated diagnostic export listener checkpoint

On 2026-08-31, source commit
`19a750f79265ec4e6d578b959fb68902739537e2` completed the first caller-bound
sealed-window export listener. The `bli-kvm-diagnostic-export-listener` crate
is not linked into the collector, another process, a fixed socket, service,
package, client, API, UI, or device runtime.

The listener reuses the existing fixed-name resolver and connection-bound
Tokio `peer_cred` adapter. Only the semantic `PlatformBroker` role becomes an
opaque export-consumer token; server, video, control, update, root, collector,
unknown, wrong-group, missing-PID, and invalid-PID peers fail closed. Numeric
PID/UID/GID values never leave the adapter.

One single-use owner accepts only a caller-provided Unix listener and a shared
`Arc<DiagnosticCollectorRuntime>`. It has two non-waiting connection permits;
an authenticated third connection is dropped rather than delaying the accept
loop. Every connection reads exactly one 16-byte request within two seconds,
writes at most one bounded response within five seconds, and then closes.
Invalid, partial, unauthorized, and over-capacity peers receive no response.

Snapshot or acknowledgement completes against the runtime before response I/O.
The owned fixed-size snapshot is moved out of the runtime clone, so no collector
mutex is held while encoding or writing up to 8,216 bytes. An applied
acknowledgement is never converted into an explicit unavailable response if
encoding fails; the connection closes and preserves delivery ambiguity for the
future consumer. Shutdown stops accepting and joins at most two tasks within
their fixed I/O deadlines.

Nine listener tests cover exact sealed/no-sealed/unavailable reads, every
acknowledgement outcome, binding propagation, one-exchange closure,
unauthorized no-state-access behavior, malformed and partial requests,
non-waiting connection pressure, stalled snapshot writes after runtime-lock
release, stalled applied-ack ambiguity, bounded shutdown, and single-use
ownership. The updated peer-adapter suite has 25 tests, including the exact
export-consumer role. The listener suite passed 20 consecutive direct runs.

The full host suite passed with 138 Python tests plus locked workspace format,
check, test, and Clippy; source, packaging, and systemd boundaries; Python
compilation and BasedPyright; production TypeScript build; ESLint; and
Prettier. LSP diagnostic requests for the new listener and updated adapter hit
the project server's 100-document limit, so LSP diagnostics are not claimed;
compiler and Clippy checks with warnings denied passed.

Locked metadata contains 158 packages: 50 workspace and 108 external, with no
missing external license expression. The listener adds no external package.
`cargo audit --deny warnings` loaded 1,233 advisories and accepted the exact
graph.

## Independent AArch64 builds

Two clean independent workspace/bundle builds produced byte-identical
4,796,617-byte prototype archives:

`60139de99412a90ee8f68b6912513f79efdcf9132036e2f18d43700f2eef78b5`

The listener is absent from the archive. Its manifest identifies source
`19a750f` and retains `releaseAccepted=false` and `deviceAccepted=false`.

The 130,522-byte listener rlibs were byte-identical:

`aa4300915d8652b1803af03052c0a05d925bf2b402c4e8b125916e4049669a1e`

Their 117,368-byte `lib.rmeta` and 11,984-byte ThinLTO members matched with
SHA-256, respectively:

`ee30f4747a790684b67c0016c8cc1ac9c3d97651f24b025bc17d0356d87d7b20`

`5987b29a2008de1ba37b1dc0f8a3d9ae7fd2ac5ea6db0a1b536a3577ebfadcd8`

The updated 293,396-byte peer-adapter rlibs matched:

`75f7d4565d1bef734df4eeb4bc8677069046714494724188ed704603cadd9b4f`

Their 265,016-byte metadata and 26,188-byte ThinLTO members matched:

`94d7043eb966f44d153e14c8ec20677aeaefde0add90be60ab21bbcf8cb4cb63`

`c7fdf846ab81e422b5ea234e1cc128486acb133f79ff343d7c4e70a1deb5ec02`

The updated 102,356-byte window-policy rlibs matched:

`96aa83ee0563c43c7102db4707517fef4f98fb76dce60dfcc75a8fdcc6836351`

Their 87,800-byte metadata and unchanged 13,600-byte ThinLTO members matched:

`0395538575c7f22b92e6d6074161aafae6f01d13fb225426aedf198772651138`

`dd6bb55dec286df14c36e00eae375084419defe1dba34e0b7daa4311b50cf1cb`

The export-protocol and lower export rlibs also remained byte-identical. Direct
comparisons passed for both archives and all five listener/adapter/window/
protocol/export rlib pairs.

No device test was run because the listener owns no path and no process links
it. The next checkpoint must bind a second fixed collector-owned socket, run
producer and export loops over the same runtime, prove exact platform-broker
authentication and producer independence under stalled export I/O, and restore
both sockets and all temporary state on H616. Persistence, export client,
platform-broker routing, HTTP, and UI remain later gates.

The unshipped collector subsequently linked this listener and added the fixed
second socket. That process-level source is tracked separately and does not
change the standalone evidence above.
