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
