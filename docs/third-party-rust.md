# Rust dependency baseline

The initial `Cargo.lock` was resolved with Rust/Cargo `1.95.0` on 2026-08-30.
At the signed-application-verifier checkpoint, the full locked graph contains
124 packages: 21 workspace packages and 103 external packages. Every external
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
successfully against all 124 locked packages on 2026-08-30.

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
