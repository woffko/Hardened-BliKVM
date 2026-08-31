# Unlinked fan supervisor checkpoint

On 2026-08-31, source commit
`9979c77367932dc5b4d16789c8028890db91a7a7` completed the mock-backed fan
supervisor slice. No concrete actuator exists and the crate is not linked into
any shipped process.

The full host suite passed, including workspace formatting, check/test/clippy,
source and packaging boundaries, 63 Python tests, BasedPyright, production
TypeScript build, ESLint, Prettier, and LSP diagnostics. Nine supervisor tests
cover:

- mandatory confirmed 100-percent startup before sample acceptance;
- changed-command application and duplicate suppression;
- normal-command failure followed by confirmed and permanently latched
  maximum cooling;
- command-plus-fallback failure, blocked sample processing, repeated explicit
  retry, and latched recovery;
- startup-failsafe failure and retry;
- policy/monotonic-clock errors never reaching the actuator;
- every new supervisor restarting from maximum cooling;
- controlled shutdown re-confirming and latching maximum cooling;
- controlled-shutdown failure marking actuation uncertain.

Two independent AArch64 workspace/bundle builds produced byte-identical
4,777,083-byte prototype archives:

`1aa684eb649498f18482cf0effc06e5b3286bf1574dd970f19d1a554a6698a65`

The supervisor rlib archives differed in non-shipped Rust metadata. Both had
the same single codegen member; extracted AArch64 ThinLTO LLVM bitcode was
byte-identical at 2,212 bytes:

`59f7e969a36316eeada61dd88f8f1d073b97db31a43f6da9578d49e37d0beaf2`

Neither fan-policy nor fan-supervisor is present in the prototype archive.
Archive equality proves that building the unlinked crates did not introduce
shipped-bundle nondeterminism; object equality separately proves repeatable
supervisor codegen.

No device or fan test was run. The supervisor can only improve controlled
software lifecycle behavior; it cannot guarantee output after SIGKILL, process
crash, kernel failure, watchdog reset, or power loss. Concrete-driver identity,
polarity/frequency/duty, exclusive ownership, readback/stall behavior,
kernel/hardware failsafe, and physical thermal soak remain mandatory.
