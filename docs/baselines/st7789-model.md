# Pure ST7789 transaction model checkpoint

On 2026-08-31, source commit
`8bf6784a50ce9d48c069c8e2dea9dcf001874661` completed the first pure Rust
ST7789 transaction/render slice. The `bli-kvm-st7789-model` crate is `no_std`,
forbids unsafe code, has no dependencies, and is not linked into any server,
broker, package, or device runtime.

The command model is pinned to the H616 vendor path at
`blikvm/blikvm@3e34c04a1dfa84636050585d12d5e585098258ea`. The effective
`LCD_1IN3_Display` path programs:

- column-address command `0x2a` plus big-endian start and inclusive end;
- row-address command `0x2b` plus big-endian start and inclusive end;
- memory-write command `0x2c`;
- RGB565 pixel data in MSB-first order, normally in 240-pixel rows.

The pure model makes command and four-byte address-data phases distinct. It
rejects zero-sized windows and every coordinate or extent outside 240x240; it
never clamps, wraps, or underflows. Typed points support bijective 0/90/180/270
degree transforms. RGB888 conversion deterministically truncates to RGB565.
The pixel stream accepts at most 240 pixels per chunk, requires an exact output
buffer length, tracks the exact window pixel count, rejects overrun and
incomplete completion, and does not mutate stream state on rejected input.

Seven focused Rust tests cover:

- exact full and partial address programs and typed command/data phase order;
- every valid one-pixel origin plus all right/bottom edge extents and their
  one-pixel overflow rejection;
- zero, off-screen, and integer-extreme windows;
- all 57,600 points through every rotation and inverse;
- canonical RGB565 colors and byte order;
- exact multi-chunk completion and rejection without state changes;
- a complete 57,600-pixel screen in 240 bounded row chunks.

The full host suite passed with 81 Python tests plus workspace
format/check/test/clippy, source and packaging boundaries, BasedPyright,
production TypeScript build, ESLint, and Prettier. The project LSP session had
already reached its 100-document limit, so LSP diagnostics are not claimed for
this new file; `cargo check` and Clippy with warnings denied both passed.

Two clean independent AArch64 workspace/bundle builds produced byte-identical
4,777,162-byte prototype archives:

`67e97b67972fb362d53bcdabfc154c72df1281a9e11bb90072849823ffd37f43`

The unshipped model rlibs were byte-identical at 95,656 bytes:

`043e6848b87c85136db708387355a55bc8abbfdd9c66a7384267d540a9e78ad2`

Each rlib contained `lib.rmeta` and one codegen member. The extracted AArch64
ThinLTO LLVM bitcode was byte-identical at 3,436 bytes:

`2f47200800f51ff8111172f5a9720c273b8396fb0ce4f44349efdaa03ade36e9`

The model is absent from the prototype archive. Archive equality proves that
building the unlinked crate did not change shipped development output; rlib and
codegen equality separately prove repeatable model output.

No device test was run for this slice because the model has no I/O. It does not
encode controller initialization, reset/backlight sequencing, SPI mode or
frequency policy, process ownership, text/fonts, user-visible content, retries,
timeouts, or crash behavior. Existing GPIO259/260/262 ownership still blocks a
concrete worker. The OLED/display capability remains unavailable until the
owner-handoff and physical gates in `device-oled-topology.md` pass.
