# Host-only fan policy checkpoint

On 2026-08-31, source commit
`09cd0410470e8e46fac11c372622a3f5551180cb` completed the first pure fan-policy
slice. It does not contain or authorize a GPIO/PWM backend and is not linked
into any shipped process.

The full host suite passed, including workspace formatting, check/test/clippy,
source and packaging boundaries, 62 Python tests, BasedPyright, production
TypeScript build, ESLint, Prettier, and LSP diagnostics. Eight focused Rust
tests cover:

- maximum-cooling startup state;
- on/off hysteresis and minimum dwell;
- immediate maximum cooling for unavailable, invalid, timed-out, and critical
  sensor inputs;
- exact timeout transition;
- monotonic duty curves and exhaustive bounded commands across reachable modes
  and boundary temperatures;
- invalid threshold, timing, timeout, minimum-duty, and curve configurations;
- an opaque 1-through-100 duty type that cannot represent zero or values above
  100;
- monotonic-clock rollback rejection without state mutation.

Two clean independent AArch64 workspace/bundle builds produced byte-identical
4,776,984-byte prototype archives:

`ab2495c9aa47ce03b8c2314775fc1bcc11ada3a87acfa810c20b3b21a62bf7da`

The fan-policy rlib archives themselves differed because of non-shipped Rust
metadata. Both contained the same single codegen member. Extracting that member
from each build produced byte-identical 11,120-byte AArch64 ThinLTO LLVM
bitcode:

`fd54405c937b4361e933a8a830ede8ca2f343a7b1f3adb69923fa78c6ae92982`

The prototype archive does not contain this unlinked library. Archive equality
proves that adding and building the policy did not introduce nondeterminism into
the shipped development bundle; object equality separately proves repeatable
fan-policy codegen.

The H616 fan capability remains unavailable. No device test was run because no
runtime consumes the policy and no fan/GPIO/PWM action is permitted by this
checkpoint. Pin identity, polarity, PWM frequency, minimum spinning duty,
exclusive ownership, startup/crash/stop application of the failsafe command,
sensor/stall fault injection, and physical thermal soak remain mandatory before
any wiring or capability promotion.
