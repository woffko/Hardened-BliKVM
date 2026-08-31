# Pure display-button policy checkpoint

On 2026-08-31, source commit
`2c4e882fc515e0c7fa49139b3eca4d522a1c6326` completed the first pure Rust
display-button policy slice. The `bli-kvm-display-button-policy` crate is
`no_std`, forbids unsafe code, has no dependencies, and is not linked into any
server, broker, package, GPIO reader, or display runtime.

At pinned vendor revision
`blikvm/blikvm@3e34c04a1dfa84636050585d12d5e585098258ea`, the H616 loop reads
source-associated offset 257 every 100 ms. It treats high as active, invokes one
display wake cycle after five consecutive active samples, resets the counter,
and therefore repeats approximately every 500 ms while held. It has no separate
release latch or monotonic-time contract.

Those details are source evidence, not physical timing or polarity acceptance.
The Hardened policy requires callers to supply active-high or active-low
polarity and explicit bounded debounce/hold timing; it has no defaults. It uses
caller-supplied monotonic milliseconds rather than sample counts. Startup input
is unsettled and never armed merely because the process started. A button held
at startup cannot emit; a debounced inactive level must arm the policy first.

After a debounced press and configured hold threshold, the policy emits exactly
one semantic `WakeDisplayCycle` intent. It remains latched until a debounced
release. Raw transitions and uncertain debounce windows cannot emit, short
bounce preserves the current stable gesture, and clock rollback is rejected
without state mutation.

Seven focused Rust tests cover:

- invalid and maximum explicit timing configurations;
- exact debounce and hold thresholds;
- one event per release-to-press cycle and no repeats under a long hold;
- startup-held input requiring release and a new press;
- press and release bounce without uncertain events or rearming;
- identical one-shot behavior for active-low configuration;
- clock rollback plus all 4,096 twelve-sample binary patterns, asserting that
  no second event occurs without a stable release.

The full host suite passed with 82 Python tests plus workspace
format/check/test/clippy, source and packaging boundaries, BasedPyright,
production TypeScript build, ESLint, and Prettier. The project LSP session
remained at its 100-document limit, so LSP diagnostics are not claimed for this
new file; `cargo check` and Clippy with warnings denied both passed.

Two clean independent AArch64 workspace/bundle builds produced byte-identical
4,777,088-byte prototype archives:

`4a2c92d0ae448ab5a99cb8fcf64b1a78f2493dd767395f18c9007dd852f2cf5f`

The unshipped policy rlibs were byte-identical at 64,124 bytes:

`c3bf66dc217d82de5be04552691881bd7a1e03eba9e0b3c163da385a2fa60be6`

Each rlib contained `lib.rmeta` and one codegen member. The extracted AArch64
ThinLTO LLVM bitcode was byte-identical at 3,596 bytes:

`c4f70878d31abf33429d67285a6fc6a01688f66677e8307b2f382a4d2553d156`

The policy is absent from the prototype archive. Archive equality proves that
building the unlinked crate did not change shipped development output; rlib and
codegen equality separately prove repeatable policy output.

No GPIO value or physical button was tested. The policy cannot prove offset,
pull resistor, electrical polarity, sampling cadence, debounce duration, wake
semantics, or interaction with the existing display owner. Offset257 remains
unused input in metadata only, and both the display-button observation and
OLED/display capabilities remain unavailable.
