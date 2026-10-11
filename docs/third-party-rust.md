# Rust dependency baseline

The initial `Cargo.lock` was resolved with Rust/Cargo `1.95.0` on 2026-08-30.
At the signed-application-verifier checkpoint, the full locked graph contains
134 packages: 30 workspace packages and 104 external packages. Every external
package declares one or more of the following permissive license expressions:

- MIT;
- Apache-2.0, including the LLVM exception where declared;
- BSD-3-Clause;
- BSL-1.0;
- LGPL-2.1-or-later as one option in a disjunctive license expression;
- Unlicense;
- Unicode-3.0.

No package reports a missing license expression in the current graph.
`cargo audit --deny warnings` loaded 1,226 RustSec advisories and completed
successfully against all 134 locked packages on 2026-08-30. The recovery-input
validator adds one workspace package and no external dependency.

The KVMD client makes existing Axum-transitive HTTP crates (`http`, `hyper`,
`hyper-util`, `http-body-util`, and `bytes`) direct dependencies. Only
`try-lock` and `want` were newly added to the lock graph. No external package
reported a missing license expression.

The signed application verifier adds 17 external packages to the previous
locked graph. Its direct additions are `ed25519-dalek` 3.0.0, `sha2` 0.11.0,
and `tar` 0.4.46 with default features disabled; `libc` was already locked.
The remaining additions are their cryptographic, deterministic-archive, and
build-time transitive dependencies. The complete locked graph was fetched and
then successfully resolved with `cargo metadata --locked --offline`.

The update runtime adds `zeroize` 1.9.0 with default features disabled for
compiler-resistant clearing of one-use health-token buffers. It is the only
new external package added by the runtime and supervisor checkpoints; the
remaining additions are workspace crates.

The system-image verifier reuses the already locked Ed25519, SHA-256,
deterministic tar, Serde, libc, and error crates. Its production dependency
graph does not depend on the application verifier crate; trust-domain
separation is enforced independently on both sides.

The recovery layout validator reuses only the already locked Serde, JSON,
SHA-256, and error crates. It adds no device, partitioning, bootloader, or
system-call dependency.

The boot-control model reuses that same Serde/JSON/SHA-256/error set and adds
no filesystem, device, bootloader, randomness, network, or system-call crate.

The recovery-input validator also reuses only Serde/JSON/SHA-256/error crates;
it does not embed a license parser, SPDX validator, downloader, or scanner.

The unlinked diagnostic peer adapter adds `nix 0.30.1` with default features
disabled and only `user` enabled. Its only new transitive locked package is the
build helper `cfg_aliases 0.2.2`; both declare MIT. Existing Tokio, libc,
bitflags, and cfg-if packages are reused. Locked offline metadata at source
commit `b62d827` resolves 152 packages: 46 workspace and 106 external, with no
missing external license expression. The adapter is not present in a shipped
binary or prototype archive. `cargo audit --deny warnings` refreshed and loaded
1,233 RustSec advisories, then accepted all 152 locked dependencies on
2026-08-31.

The transient diagnostic listener reuses the already locked `rand_core 0.6.4`
and `getrandom 0.2.17` entropy path for a process-local nonzero instance
binding. Its host-only tests reuse the already locked `tempfile`. Source commit
`8905093` therefore adds no external package: locked offline metadata remains
152 total, 46 workspace, and 106 external packages, with no missing external
license expression. The refreshed 1,233-advisory RustSec scan accepted the
exact graph.

The unshipped diagnostic collector adds `signal-hook 0.3.18` and
`signal-hook-registry 1.4.8` for safe SIGTERM/SIGINT handoff without expanding
the shared Tokio feature set. Both are permissively licensed; the already
locked `errno` and `libc` dependencies are reused. At collector/gate source
`9997c5b`, locked metadata resolves 155 packages: 47 workspace and 108
external, with no missing external license expression. The refreshed
1,233-advisory RustSec scan accepted the exact graph. The collector and its
templates remain outside the prototype archive.

The diagnostic producer client reuses only the existing Tokio and
diagnostic protocol/export graph. Source `49173a7` adds one workspace package
and no external package: locked metadata is 156 total, 48 workspace, and 108
external packages with no missing external license expression. The refreshed
1,233-advisory RustSec scan accepted the exact graph.

The bounded diagnostic reporter added inside that same unshipped crate at
source `5ccd68b` reuses Tokio synchronization, task, and time support already
present in the exact lock graph. It adds no workspace or external package:
locked metadata remains 156 total, 48 workspace, and 108 external packages
with no missing external license expression. The refreshed 1,233-advisory
RustSec scan accepted the unchanged graph.

Control-broker integration at source `a143a75`, followed by the direct
control-broker `thiserror` removal at `bc2e5f0`, adds no package. Locked
metadata remains 156 total, 48 workspace, and 108 external packages. The
refreshed 1,233-advisory RustSec scan accepted the exact graph.

The pure diagnostic sealed-window export protocol adds one workspace package
and reuses only `bli-kvm-diagnostic-export`. It adds no external package:
locked metadata is 157 total, 49 workspace, and 108 external packages with no
missing external license expression. The refreshed 1,233-advisory RustSec scan
accepted the exact graph.

The unlinked diagnostic export listener adds one workspace package and reuses
only the existing export/protocol/window/peer-adapter/Tokio graph. It adds no
external package: locked metadata is 158 total, 50 workspace, and 108 external
packages. The refreshed 1,233-advisory RustSec scan accepted the exact graph.

Linking the export listener into the existing unshipped collector adds no
workspace or external package and no dependency version.

The unlinked diagnostic export client adds one workspace package and reuses
only the existing export-protocol and Tokio graph. It adds no external package:
locked metadata is 159 total, 51 workspace, and 108 external packages with no
missing external license expression. The refreshed 1,233-advisory RustSec scan
accepted the exact graph at source `3f1984d`.

The default-off Platform Broker diagnostic route reuses only existing workspace
crates plus the already locked nix/Tokio/Serde graph. It adds no workspace or
external package: locked metadata remains 159 total, 51 workspace, and 108
external packages with no missing external license expression. The refreshed
1,233-advisory RustSec scan accepted the exact graph at final source `3461d77`.

The pure encrypted diagnostic journal adds one workspace package and nine
external RustCrypto packages: `chacha20poly1305` 0.11.0, `chacha20` 0.10.2,
`poly1305` 0.9.1, `aead` 0.6.1, `cipher` 0.5.2, `inout` 0.2.2, `cmov` 0.5.4,
`ctutils` 0.4.2, and `universal-hash` 0.6.1. All are MIT/Apache-2.0 licensed.
Locked metadata is 169 total, 52 workspace, and 117 external packages with no
missing external license expression. Default alloc/getrandom features are
disabled and no RNG dependency is enabled. A refreshed 1,233-advisory RustSec
scan accepted the exact graph at source `11ee117`.

The atomic encrypted diagnostic store adds one workspace package and no
external dependency. It reuses the already locked journal, libc, nix,
`rand_core`/OS entropy, and host-test-only tempfile graph. Locked metadata at
source `2efe1f7` contains 170 packages: 53 workspace and 117 external, with no
missing external license expression. A refreshed 1,233-advisory RustSec scan
accepted the exact graph. The store remains unlinked and absent from the
prototype archive.

The diagnostic key loader adds one workspace package and no external
dependency. It reuses the already locked journal, libc, nix, subtle, zeroize,
and host-test-only SHA-256/tempfile graph. Making subtle a direct journal
dependency supports constant-time duplicate-key-material rejection without a
lockfile version change. Locked metadata at source `b3266c3` contains 171
packages: 54 workspace and 117 external, with no missing external license
expression. A 1,233-advisory RustSec scan accepted the exact graph. The loader
remains unlinked and absent from the prototype archive.

The diagnostic maintenance boundary adds one workspace package and no external
dependency. It reuses only the workspace key-loader/store graph; its device
gate is Python/shell development tooling and is not packaged. Locked metadata
at source `dfb47a2` contains 172 packages: 55 workspace and 117 external, with
no missing external license expression. A 1,233-advisory RustSec scan accepted
the exact graph. The maintenance executable, units, tmpfiles source, loader,
store, and journal remain absent from the prototype archive.

The diagnostic coordinator adds one `no_std` workspace package and no external
dependency. It reuses only the diagnostic journal in production; the typed
diagnostic export crate is a test-only workspace dependency. It remains
unlinked and absent from every server, broker, unit, package, and prototype
archive. The locked graph contains 173 packages: 56 workspace and 117 external,
with no missing external license expression. A 1,233-advisory RustSec scan at
source `945eb19` accepted the exact graph.

The diagnostic persistence adapter adds one workspace package and no external
dependency. It reuses only the accepted coordinator, typed export/client,
journal, key-loader, and store graph; Tokio is test-only for cancellation
checks. The adapter remains unlinked and absent from every executable, unit,
package, and prototype archive. The locked graph contains 174 packages: 57
workspace and 117 external, with no missing external license expression. A
1,233-advisory RustSec scan at source `57959b0` accepted the exact graph.

The synthetic diagnostic persistence gate adds one unshipped workspace binary
and no external dependency. It reuses the accepted diagnostic graph and Tokio;
its host rollback tool uses only Python's standard library plus existing local
helpers. Neither binary nor unit enters the prototype archive. The locked graph
contains 175 packages: 58 workspace and 117 external, with no missing external
license expression. A 1,233-advisory RustSec scan at source `7bef2bc` accepted
the exact graph.

The authenticated current-only diagnostics view adds no workspace or external
dependency. The server reuses its existing core and read-only broker client;
the broker protocol is referenced only by server tests to provide an exact
framed inventory fixture.

This is an inventory checkpoint, not final legal acceptance. Before a release:

1. run the inventory from the exact locked dependency graph;
2. retain required license and notice text;
3. run `cargo audit` with a current advisory database;
4. add a policy checker such as `cargo-deny` and verify its configuration;
5. include native, npm, kernel, firmware, and image dependencies in the final
   SPDX SBOM.

Reproduce the raw Rust inventory with:

```sh
cargo metadata --locked --offline --format-version 1
```

The QUIC gateway (plan of 2026-10-01, `docs/design/quic-transport-v1.md`)
adds `wtransport` 0.7.2 with the features `quinn`, `ring` and `self-signed`
as its one direct addition; `quinn` 0.11.12, `rustls` 0.23.45, `ring`
0.17.14 and `rcgen` 0.14.10 come with it. The lock gained 94 external
packages and two workspace crates (`bli-kvm-quic-protocol`,
`bli-kvm-quic-gateway`). 68 of the external ones are built for the board,
40 of those (URL parsing with the Unicode tables, an X.509 parser, the
native certificate store) only because the library's features cannot leave
them out; the rest belong to other targets or to tests. The stack is a
normal dependency of `bli-kvm-quic-gateway` alone and a development
dependency of the web server, whose tests open sessions as a page does. At
the end of that plan the locked graph has 313 packages, 80 of the workspace
and 233 external, each with a declared licence expression (`ISC`,
`BSD-2-Clause`, `Zlib` as one option and `Apache-2.0 AND ISC` for `ring`
beside the ones above).

## RustSec scan for 0.1.0-alpha.10 (2026-10-02)

`cargo audit --deny warnings` on the lock of the release (313 packages)
reports one advisory: RUSTSEC-2026-0009 in `time` 0.3.45, a stack exhaustion
when input from outside is parsed with the RFC 2822 format; 0.3.47 fixes it.
`time` comes with the QUIC stack (`wtransport`, `rcgen`, `x509-parser`,
`asn1-rs`, `yasna`) and is linked only into `bli-kvm-quic-gateway`. Neither
the gateway nor these crates parse that format: the gateway makes its own
certificate and `x509-parser` only writes dates with it. The release ships
the locked version; the lock moves to the fixed one with the next build of
the update-runtime package, which carries the gateway.

## RustSec scan for 0.1.0-beta.1 (2026-10-04)

The lock of the release has 315 packages, 82 of the workspace and 233
external. Since alpha.10 it gained two workspace packages, the video engine's
encoder crate (`bli-kvm-ve-encoder`) and its lab tool (`bli-kvm-ve-lab`, which
no package carries), and no external one; every external package declares a
licence expression. `cargo audit --deny warnings`, run without fetching (the
advisory database as fetched on 2026-10-02, 1,288 advisories), reports the one
advisory of alpha.10: RUSTSEC-2026-0009 in `time` 0.3.45, linked only into
`bli-kvm-quic-gateway`, where its fault is not reachable (above). The section
above expected the fixed version with the next build of the update-runtime
package. That build, `0.1.0~dev36` (it added the encoder driver), and this
release's runtime, whose programs are the same bytes, still lock 0.3.45:
moving the lock needs the crate registry, which neither used. The release
notes say so.

## RustSec scan for 0.1.0-beta.2 (2026-10-06)

The lock is beta.1's (315 packages). `cargo audit --deny warnings`, run without
fetching (the database as fetched on 2026-10-02, now 1,290 advisories), reports
the same one advisory, RUSTSEC-2026-0009 in `time` 0.3.45, as above.

## RustSec scan for 0.1.0-beta.3 (2026-10-07)

The lock is beta.2's (315 packages; no dependency changed). `cargo audit
--deny warnings`, run without fetching (the database as fetched on 2026-10-07,
its last advisory commit of that morning, 1,293 advisories), reports the same
one advisory, RUSTSEC-2026-0009 in `time` 0.3.45, linked only into
`bli-kvm-quic-gateway`, where its fault is not reachable (above). The card
image and the RAW system bundle of this release carry the board's update
runtime `0.1.0~dev36`, whose gateway was built from that lock.

## RustSec scan for 0.1.0-beta.4 (2026-10-11)

The lock has beta.3's 315 packages; since then only the board daemon gained
a dependency on the workspace's own `bli-kvm-core`. `cargo audit --deny
warnings`, run without fetching (the database as fetched on 2026-10-09, its
last advisory commit of that morning, 1,296 advisories), reports the same
one advisory, RUSTSEC-2026-0009 in `time` 0.3.45, linked only into
`bli-kvm-quic-gateway`, where its fault is not reachable (above). The card
image and the RAW system bundle of this release carry the board's update
runtime `0.1.0~dev38`, whose gateway (QUIC protocol 2) was built from that
lock.

## RustSec scan for 0.1.0-beta.5 (2026-10-11)

An application release; the lock is beta.4's (315 packages, unchanged).
`cargo audit --deny warnings`, run without fetching (the same database,
1,296 advisories), reports the same one advisory, RUSTSEC-2026-0009 in
`time` 0.3.45, linked only into `bli-kvm-quic-gateway`, which is part of
the system (beta.4's update runtime `0.1.0~dev38`), not of this bundle.
