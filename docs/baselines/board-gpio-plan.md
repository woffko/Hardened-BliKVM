# Board GPIO request plan

Status: host-only, no I/O. Part of the vendor-to-Rust board migration
(`docs/vendor-rust-migration.md`), Phase 2 groundwork.

`crates/bli-kvm-platform-bli/src/gpio.rs` adds a pure line-request planner on
top of the source-derived H616 profile. It performs no device access and opens
no GPIO chip.

## What it encodes

- Current live ownership from the read-only line baseline: KVMD owns ATX
  power, reset and both ATX LED inputs; legacy sysfs owns the activity LED and
  fan PWM; the display button offset is an unused input. No line is claimed by
  the hardened Rust board daemon yet.
- A fixed consumer label (`hardened-blikvm-board`) that is never taken from
  caller input.
- Direction and polarity from the reviewed profile, not from callers.

## What it refuses

- Any request by `HardenedBoard` for a line it does not currently hold, so the
  Rust daemon cannot take over a KVMD or legacy sysfs line before the reviewed
  physical handoff.
- Unknown roles, duplicate roles and plans larger than the seven reviewed
  lines.

## Transactional driver seam

`crates/bli-kvm-platform-bli/src/gpio_driver.rs` defines the `GpioDriver`
interface the board daemon will use and the ownership transaction:
`apply_plan` requests every planned line or releases the already-requested
lines in reverse order before reporting the failure, and `release_plan`
releases in reverse request order. A failed rollback is reported distinctly so
a half-acquired line set is never silently accepted. The seam is exercised with
a deterministic mock; the concrete libgpiod adapter is a later, physically
gated slice.

## Not in this slice

- The libgpiod driver, chip open, line request ioctls and pulse timing.- ATX execution: KVMD remains the executor over its fixed waiting routes until
  a later, physically gated handoff.
- Fan, LED, button, OLED/ST7789, RTC, and serial-switch drivers.

Every firmware/hardware fact stays a source hypothesis until the physical gate
confirms line identity on the designated H616 board.
