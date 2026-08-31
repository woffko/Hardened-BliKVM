# Pure encrypted diagnostic journal checkpoint

On 2026-09-01, source commit
`11ee117991a75643660a172d3e410834ac8286fa` completed the first pure encrypted
diagnostic journal and write-before-ack reconciliation model. The
`bli-kvm-diagnostic-journal` crate is `no_std`, forbids unsafe code, and is not
linked into the default or feature broker, collector, server, another process,
prototype archive, package, systemd unit, API, UI, or device runtime.

The accepted threat model and exact format are in
`../design/diagnostic-journal-v1.md`. V1 protects fixed typed diagnostics from
offline disclosure and undetected file tampering. It explicitly does not
protect against compromised root/live process memory and cannot detect replay
of two complete older valid copies without a separately surviving monotonic
floor.

## Cryptographic boundary

Each copy is exactly 16,552 bytes: a 64-byte authenticated clear header, one
fixed 16,472-byte ciphertext, and one detached 16-byte tag. The plaintext has a
32-byte canonical header and two fixed 8,220-byte slots, so offline readers do
not learn entry count or bundle length.

V1 uses RustCrypto `XChaCha20Poly1305` with a 32-byte key and 24-byte nonce.
`chacha20poly1305` default alloc/getrandom features are disabled; only key
zeroization is enabled. The crate performs no RNG and accepts only a nonzero
caller-supplied nonce. New writes use the active nonzero key ID, reject nonce
reuse against retained copies under that key, and can read one distinct
previous key ID during rotation. The whole clear header is AAD.

The format rejects zero keys/nonces, wrong or unavailable keys, invalid
magic/schema/algorithm/reserved/length/generation, ciphertext/tag/header
tampering, noncanonical plaintext, duplicate bindings, multiple nonterminal
entries, same-generation divergence, generation gaps, and selection below a
caller-supplied rollback floor. Initialization and updates encrypt once and
write identical bytes to A/B; repair copies selected authenticated bytes and
does not consume another nonce.

Keys and cipher material plus transient encode/decrypt plaintext buffers are
zeroized. Key/key-ring types implement neither `Clone` nor `Debug`. Entry,
journal, selected-copy, mutation, and plan `Debug` output includes only bounded
metadata and never bundle bytes. Errors carry fixed enums only.

## Reconciliation boundary

At most one `pending_ack` or `ack_ambiguous` entry exists. Only terminal
`acknowledged` or `orphaned` entries can be evicted, oldest first. A new sealed
snapshot first produces a private mutation; no ack action exists until the
resulting encrypted A/B generation is written, read back, authenticated, and
selected by the caller.

A durably selected pending match permits one acknowledgement. Transport
ambiguity first plans a durable ambiguous state and then a collector reread.
The same sealed snapshot plans a durable return to pending before another ack
can be authorized. Mismatch requests reread without mutation. A different
binding or an absent window that could also mean collector restart becomes
`orphaned`; only an explicit applied response becomes `acknowledged`.

Twelve focused tests cover exact sizes and stable format digest, key/nonce/key
ring guards, ciphertext plaintext-hiding, tamper/wrong/missing key failures,
new-window write-before-ack, ambiguous/retry/no-sealed/different-binding flows,
applied/mismatch results, retention, nonce reuse, previous-key rotation,
degraded repair, generation gap, split brain, rollback floor, and redacted
debug output.

The complete source host job `4d0041b1d47f4addb79e00b8d13a1cb4` exited zero.
It passed locked workspace format/check/test/Clippy, all-feature broker checks,
source/packaging/systemd boundaries, 141 Python tests, compilation and
BasedPyright, production TypeScript/Vite, ESLint, and Prettier. Focused tests,
Clippy, rustdoc with warnings denied, a refreshed RustSec scan, and the stable
format vector also passed. LSP diagnostics twice hit the project 100-document
limit, so compiler/Clippy/tests are the semantic evidence.

The lock graph contains 169 packages: 52 workspace and 117 external, with no
missing external license expression. Nine RustCrypto packages were added; all
are MIT/Apache-2.0 licensed. `cargo audit --deny warnings` loaded 1,233
advisories and accepted the exact graph.

## Independent AArch64 builds

Jobs `d47d7d240afc42b296996a138369e958` and
`479dafeba52b4dbe84cc3a232b8c057f` used independent target/bundle directories
under `/mnt/d/debug/blikvm/`. They produced byte-identical 4,798,583-byte
default prototype archives:

`102970ddf5d0a75858ffc446ccfb1f5b01c970127d04a54ba2de6d8a58413490`

The archive contains no journal or cryptographic artifact. Its manifest
identifies exact source `11ee117`, a committed development checkpoint, and
retains `releaseAccepted=false` and `deviceAccepted=false`.

The 284,816-byte journal rlibs were byte-identical:

`48634bee3f10dfba203a786e1c9cb973f95e80cad005ba6795d6955b8f9521e3`

Their 170,008-byte standalone metadata matched:

`8e33c63cc94472ea753d7096f20d61bfc8f62c2c1a8591322bd6fb792df8fcec`

Their 111,848-byte ThinLTO members matched:

`27b658e09d5ac00ed3f63fe8a367399c610bdbe5aa7cc6a1765d5efb54a7ef46`

The direct `rlib`, standalone metadata, and ThinLTO comparisons also matched
for `chacha20poly1305`, `chacha20`, `poly1305`, and `aead`. Recorded rlib hashes
are respectively:

- `58f74ee5cd243a344f1e128a111292bf488504c1e1932540457ad1c20aff6621`;
- `392ba1811ac59f09b27c913fc58b1dc01de70b4f766ee6406f08801339caf540`;
- `a38deff3c2159b72a7eb0b72c0ca73954d18e950bb238bbc709561c707a6d8cd`;
- `5aa52152ae0fa6b9af7b22de3e9486d849c691d35e88d6252e9005531b3c192f`.

No device test was run because this pure crate has no storage path, random
source, key, process link, acknowledgement call, or runtime behavior. The
fixed-root atomic A/B filesystem store and OS-random nonce boundary were
subsequently completed at source `2efe1f7`; see `diagnostic-store.md`. A
root-provisioned systemd key credential, native store harness, external floor,
and live write-before-ack coordinator remain separate later gates.
