# Pure serial-switch codec checkpoint

On 2026-08-31, source commit
`2410e42275988bfe41f1eba3f682f633aaabd5fc` completed the first pure
serial-switch protocol slice. The `bli-kvm-serial-switch-codec` crate is
`no_std`, forbids unsafe code, has no dependencies, and is not linked into any
server, broker, package, or device runtime.

The protocol evidence is pinned to
`blikvm/blikvm@3e34c04a1dfa84636050585d12d5e585098258ea`, specifically
`src/kvmd/blikvm_switch/blikvm_switch.c`. The vendor source constructs the ten
initialized command bytes `SW1\r\nG0{port}gA`, but writes a 12-byte buffer. It
also reads status into an eight-byte buffer, uses byte two as the selected port,
and accepts a device after checking only `G` at byte zero and `A` at byte four.

The Hardened codec deliberately narrows that behavior:

- ports are a typed one-through-four enum;
- selection commands are exactly ten fully initialized bytes with no padding;
- a selection command decodes only as exact `SW1\r\nG0{1..4}gA`;
- a status frame decodes only as exact `G0{1..4}gA` and exactly five bytes;
- empty, short, trailing, padded, noisy, mutated, and out-of-range frames are
  rejected.

Fixing status bytes one and three to `0` and `g` is a fail-closed interpretation
of the complete token present in the vendor command. It is not a physical wire
capture. A future adapter must retain this distinction and cannot relax the
grammar without reviewed device evidence.

The full host suite passed, including workspace formatting, check/test/clippy,
source and packaging boundaries, 74 Python tests, BasedPyright, production
TypeScript build, ESLint, and Prettier. Six focused Rust tests cover all four
exact frames, const/no-padding encoding, numeric port bounds, exhaustive 256-byte
port positions for both decoders, fixed-byte mutation, and length/noise
rejection. Rust LSP diagnostics were empty for the final source.

Two clean independent AArch64 workspace/bundle builds produced byte-identical
4,777,150-byte prototype archives:

`72350d42ed555eff2b4761ec13231b203256be7e0fccf27571d305f17f707df6`

The unshipped codec rlib archives were also byte-identical at 35,188 bytes:

`b166a5cebbf18dee5684201e100c7c20916436d524c55027371b6480cb5bd6a1`

Each rlib contained `lib.rmeta` and one codegen member. The extracted AArch64
ThinLTO LLVM bitcode was byte-identical at 6,584 bytes:

`85479e208b89a61db14bf0d60d3fd39408e774e80ab0b640221d441fa0ea2277`

The prototype archive does not contain the codec. Archive equality proves that
building the unlinked crate did not introduce nondeterminism into the shipped
development bundle; rlib and codegen equality separately prove repeatable codec
build output.

No TTY was opened and no frame was sent to a device. The serial-switch
capability remains unavailable. Deliberate non-unique device identity, exclusive
19200/8N1 ownership, bounded I/O and timeout behavior, framing across partial or
coalesced reads, hotplug/reconnect, and physical four-port behavior remain
mandatory before any adapter or capability promotion.
