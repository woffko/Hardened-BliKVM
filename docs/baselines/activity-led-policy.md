# Pure activity LED policy checkpoint

On 2026-08-31, source commit
`012ebe358396051750dea4e60c83736642503bcc` completed the first pure Rust
activity LED policy slice. The `bli-kvm-activity-led-policy` crate is `no_std`,
forbids unsafe code, has no dependencies, and is not linked into any server,
broker, package, or GPIO runtime.

At pinned vendor revision
`blikvm/blikvm@3e34c04a1dfa84636050585d12d5e585098258ea`, the H616 loop maps
the activity LED to source-associated offset261. Every 100 ms it advances a
20-step counter: step zero writes LOW, step one writes HIGH, and the remaining
steps preserve HIGH. This creates a 100-ms LOW interval every two seconds,
independent of observed storage or network activity.

That schedule is source evidence, not accepted LED polarity or product
semantics. The Hardened policy requires explicit asserted level, period, and
pulse duration with bounded limits and a mandatory idle interval. It has no
defaults. Startup is disabled and inactive. Disabled and fault modes always
return the configured inactive level; enabling starts a fresh pulse.

The enabled phase is calculated from an absolute monotonic enable timestamp,
so delayed evaluation skips directly to the correct phase without accumulated
drift. Decisions expose command and mode changes. Clock rollback is rejected
without state mutation, and a new engine always restarts disabled/inactive.

Eight focused Rust tests cover:

- invalid period, pulse, and insufficient-idle configurations;
- exact edges of the source-shaped 2,000/100-ms schedule;
- large delayed evaluations without phase drift;
- disabled/fault inactive behavior and pulse restart after re-enable;
- active-high and active-low electrical parity;
- clock rollback without mutation;
- deterministic disabled/inactive restart;
- every millisecond across two periods for both asserted levels.

The full host suite passed with 83 Python tests plus workspace
format/check/test/clippy, source and packaging boundaries, BasedPyright,
production TypeScript build, ESLint, and Prettier. The project LSP session
remained at its 100-document limit, so LSP diagnostics are not claimed for this
new file; `cargo check` and Clippy with warnings denied both passed.

Two clean independent AArch64 workspace/bundle builds produced byte-identical
4,777,150-byte prototype archives:

`cb67017acad404c3efbc64dabea41eb659aa0b24a2ab9eed0310843ea379c2c7`

The unshipped policy rlibs were byte-identical at 63,168 bytes:

`272bd39b21ef1841cf0ea923ee2afbaf23fe83a1c7d6a5c4ad7b7844d9464489`

Each rlib contained `lib.rmeta` and one codegen member. The extracted AArch64
ThinLTO LLVM bitcode was byte-identical at 2,216 bytes:

`023be534a2f2ae86a5d0de12e156050e2a1114bb3bd2299126b5a8387509b607`

The policy is absent from the prototype archive. Archive equality proves that
building the unlinked crate did not change shipped development output; rlib and
codegen equality separately prove repeatable policy output.

No GPIO value or physical LED was tested. The policy cannot establish whether
LOW or HIGH illuminates the device, whether a heartbeat is the desired product
behavior, or whether offset261 can be released safely. The line remains a used
legacy sysfs output and the activity-LED capability remains unavailable until
owner-handoff, polarity, visible timing, crash/reboot, and physical gates pass.
