# Rust dependency baseline

The initial `Cargo.lock` was resolved with Rust/Cargo `1.95.0` on 2026-08-30.
After adding the bounded KVMD Unix-socket client, the full locked graph contains
86 external packages and four workspace packages. Every external package declared
one or more of the following permissive license expressions:

- MIT;
- Apache-2.0, including the LLVM exception where declared;
- BSD-3-Clause;
- BSL-1.0;
- LGPL-2.1-or-later as one option in a disjunctive license expression;
- Unlicense;
- Unicode-3.0.

No package reported a missing license expression in this initial graph.
`cargo audit --deny warnings` loaded 1,226 RustSec advisories and completed
successfully against all 90 locked packages on 2026-08-30.

The KVMD client makes existing Axum-transitive HTTP crates (`http`, `hyper`,
`hyper-util`, `http-body-util`, and `bytes`) direct dependencies. Only
`try-lock` and `want` were newly added to the lock graph. No external package
reported a missing license expression.

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
