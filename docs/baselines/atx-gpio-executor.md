# ATX GPIO executor

Status: host-only, no device access. Phase 2 groundwork for replacing the KVMD
ATX route with the Rust board executor.

`crates/bli-kvm-atx-gpio-executor` drives the reviewed ATX power/reset lines
through the platform GPIO driver seam:

- `AtxTiming` holds the power-short, power-long and reset hold times; the values
  are configuration supplied by the reviewed owner, not hardware facts.
- `AtxGpioExecutor::execute` requests the line, drives it active for the hold
  time, always drives it inactive, and always releases it, even when the active
  write fails. A failed request never writes.
- Construction rejects non-actuator lines, non-output directions and
  out-of-range timings.

`bli-kvm-platform-bli` gains `GpioDriver::write_line` (with polarity handled by
the driver) and `hardened_board_request`, which builds an actuator request for a
line owned by the hardened board after the reviewed physical handoff and refuses
every non-ATX role. Tests use a mock driver and a mock sleeper, so no real time
passes and no GPIO is touched.

## Gates

- the reviewed physical handoff that transfers ATX line ownership from KVMD;
- the hold times confirmed on the designated H616 board;
- wiring an `AtxExecutor::Gpio` variant once the owner is live.
